//! Target-aware provider access materialized into the existing connection contract.

use std::collections::{BTreeMap, BTreeSet};

use openengine_cluster_protocol::{
    ClaudeProvider, CodexProvider, ConnectionKey, DeclaredConnections, DeclaredEnvironment,
    EnvironmentVariableName, NativeV2RunValueError, NodeRuntimeBinding, RuntimePlan,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderAccessPlacement {
    Local,
    Contained,
}

#[derive(Clone, Copy)]
struct ProviderAccessContract {
    native_local: bool,
    connection_key: &'static str,
    required_fields: &'static [&'static str],
    accepted_field_sets: &'static [&'static [&'static str]],
}

impl ProviderAccessContract {
    const fn new(
        native_local: bool,
        connection_key: &'static str,
        required_fields: &'static [&'static str],
        accepted_field_sets: &'static [&'static [&'static str]],
    ) -> Self {
        Self {
            native_local,
            connection_key,
            required_fields,
            accepted_field_sets,
        }
    }
}

pub(crate) fn materialize_provider_access(
    runtime: &mut RuntimePlan,
    placement: ProviderAccessPlacement,
) -> Result<(), NativeV2RunValueError> {
    let contract = provider_access_contract(runtime);
    for binding in runtime_nodes_mut(runtime).values_mut() {
        materialize_binding(binding, contract, placement)?;
    }
    Ok(())
}

fn provider_access_contract(runtime: &RuntimePlan) -> ProviderAccessContract {
    match runtime {
        RuntimePlan::Copilot { .. } => ProviderAccessContract::new(
            true,
            "github",
            &["COPILOT_GITHUB_TOKEN"],
            &[&["COPILOT_GITHUB_TOKEN"]],
        ),
        RuntimePlan::Codex { provider, .. } => codex_contract(*provider),
        RuntimePlan::Claude { provider, .. } => claude_contract(*provider),
    }
}

fn codex_contract(provider: CodexProvider) -> ProviderAccessContract {
    match provider {
        CodexProvider::OpenAi => ProviderAccessContract::new(
            true,
            "openai",
            &["OPENAI_API_KEY"],
            &[&["OPENAI_API_KEY"], &["CODEX_API_KEY"]],
        ),
        CodexProvider::OpenRouter => ProviderAccessContract::new(
            false,
            "openrouter",
            &["OPENROUTER_API_KEY"],
            &[&["OPENROUTER_API_KEY"]],
        ),
        CodexProvider::Gateway => ProviderAccessContract::new(
            false,
            "gateway",
            &["GATEWAY_BASE_URL", "GATEWAY_API_KEY"],
            &[&["GATEWAY_BASE_URL", "GATEWAY_API_KEY"]],
        ),
        CodexProvider::Bedrock => ProviderAccessContract::new(
            false,
            "bedrock",
            &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
            &[&["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"]],
        ),
    }
}

fn claude_contract(provider: ClaudeProvider) -> ProviderAccessContract {
    match provider {
        ClaudeProvider::Anthropic => ProviderAccessContract::new(
            true,
            "anthropic",
            &["ANTHROPIC_API_KEY"],
            &[
                &["ANTHROPIC_API_KEY"],
                &["ANTHROPIC_AUTH_TOKEN"],
                &["CLAUDE_CODE_OAUTH_TOKEN"],
                &["CLAUDE_CODE_OAUTH_REFRESH_TOKEN"],
            ],
        ),
        ClaudeProvider::OpenRouter => ProviderAccessContract::new(
            false,
            "openrouter",
            &["OPENROUTER_API_KEY"],
            &[&["OPENROUTER_API_KEY"]],
        ),
        ClaudeProvider::Gateway => ProviderAccessContract::new(
            false,
            "gateway",
            &["GATEWAY_BASE_URL", "GATEWAY_API_KEY"],
            &[&["GATEWAY_BASE_URL", "GATEWAY_API_KEY"]],
        ),
        ClaudeProvider::Bedrock => ProviderAccessContract::new(
            false,
            "bedrock",
            &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
            &[&["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"]],
        ),
    }
}

fn runtime_nodes_mut(
    runtime: &mut RuntimePlan,
) -> &mut BTreeMap<openengine_cluster_protocol::NodeName, NodeRuntimeBinding> {
    match runtime {
        RuntimePlan::Copilot { nodes, .. }
        | RuntimePlan::Codex { nodes, .. }
        | RuntimePlan::Claude { nodes, .. } => nodes,
    }
}

fn materialize_binding(
    binding: &mut NodeRuntimeBinding,
    contract: ProviderAccessContract,
    placement: ProviderAccessPlacement,
) -> Result<(), NativeV2RunValueError> {
    let NodeRuntimeBinding::Agent { connections, .. } = binding else {
        return Ok(());
    };
    if supports_authored_access(connections, contract)
        || placement == ProviderAccessPlacement::Local && contract.native_local
    {
        return Ok(());
    }
    *connections = with_connection_fallback(connections, contract)?;
    Ok(())
}

fn supports_authored_access(
    connections: &DeclaredConnections,
    contract: ProviderAccessContract,
) -> bool {
    contract.accepted_field_sets.iter().any(|fields| {
        fields.iter().all(|field| {
            connections
                .environment_names()
                .any(|name| name.as_str() == *field)
        })
    })
}

fn with_connection_fallback(
    connections: &DeclaredConnections,
    contract: ProviderAccessContract,
) -> Result<DeclaredConnections, NativeV2RunValueError> {
    let mut merged = connections
        .iter()
        .map(|(key, fields)| (key.clone(), fields.iter().cloned().collect::<BTreeSet<_>>()))
        .collect::<BTreeMap<_, _>>();
    let existing = connections
        .environment_names()
        .cloned()
        .collect::<BTreeSet<_>>();
    let fallback = merged
        .entry(ConnectionKey::new(contract.connection_key)?)
        .or_default();
    for name in contract.required_fields {
        let name = EnvironmentVariableName::new(*name)?;
        if !existing.contains(&name) {
            fallback.insert(name);
        }
    }
    DeclaredConnections::new(
        merged
            .into_iter()
            .map(|(key, fields)| DeclaredEnvironment::new(fields).map(|fields| (key, fields)))
            .collect::<Result<Vec<_>, _>>()?,
    )
}

#[cfg(test)]
#[path = "provider_access/tests.rs"]
mod tests;
