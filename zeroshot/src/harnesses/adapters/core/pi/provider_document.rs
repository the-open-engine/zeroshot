//! Synthesized `models.json` for Pi lanes that carry a caller-owned endpoint.
//!
//! Pi reads its provider catalog from this file. Two behaviors decide its shape, and both were
//! confirmed against the CLI rather than inferred. A *provider-level* `api` override does not
//! retag a built-in provider's existing models, though a model definition's own `api` always
//! applies. And a `models.json` `apiKey` is honored unless the agent directory holds a stored
//! credential for that provider, which outranks it on any provider, built-in or registered.
//! `baseUrl` is honored on both. That is why the `gateway` lane registers its own provider
//! instead of retargeting `openai` — it needs to choose the wire protocol — while the `anthropic`
//! lane retargets the endpoint and keeps the built-in protocol because it needs no `api`
//! override.
//!
//! The registered model entry carries the caller's own identifier, the chosen protocol, a display
//! name, whether the caller's binding asked for reasoning, and any caller-declared context window
//! and output limit. Zeroshot never becomes a model catalog: omitted limits stay unset so Pi fills
//! in its own defaults, and nothing is ever inferred from the model identifier.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::native_v2_capsule::gateway;
use crate::native_v2_runner::{NodeRunnerError, ResolvedEnvironment};

/// Pi's provider file inside the private agent directory.
pub(super) const MODELS_FILE: &str = "models.json";

/// One provider override: which Pi provider to register, which protocol it speaks, and which
/// environment variable holds its key.
#[derive(Clone, Copy)]
pub(super) struct ProviderOverride {
    /// Pi provider name this file registers.
    pub(super) provider: &'static str,
    /// Environment field the API key is read from, referenced by Pi's `$NAME` interpolation.
    pub(super) api_key_field: &'static str,
}

/// The gateway lane registers its own provider so the caller owns the wire protocol.
pub(super) const GATEWAY: ProviderOverride = ProviderOverride {
    provider: "zeroshot-gateway",
    api_key_field: gateway::API_KEY,
};

/// The `anthropic` lane retargets Pi's built-in provider, because that lane's protocol is already
/// the one Pi speaks. No discriminant is written: a provider-level `api` override does not retag a
/// built-in provider's existing models.
pub(super) const ANTHROPIC: ProviderOverride = ProviderOverride {
    provider: "anthropic",
    api_key_field: "ANTHROPIC_API_KEY",
};

/// The `openai` lane retargets Pi's built-in OpenAI provider for the same reason. It keeps the
/// built-in Responses protocol, so a caller-owned endpoint for this lane must speak Responses; a
/// caller with another protocol uses the gateway lane, which can name it.
pub(super) const OPENAI: ProviderOverride = ProviderOverride {
    provider: "openai",
    api_key_field: "OPENAI_API_KEY",
};

/// Wire protocols Pi can be asked to speak. The caller names one; Zeroshot never guesses it from
/// the model identifier, because an identifier is opaque and provider-owned.
pub(super) const SUPPORTED_APIS: [&str; 3] = [
    "openai-responses",
    "openai-completions",
    "anthropic-messages",
];

#[derive(Debug, thiserror::Error)]
pub(super) enum PiProviderDocumentError {
    #[error("Pi provider configuration could not be serialized: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("Pi provider configuration could not be created: {0}")]
    Write(#[source] std::io::Error),
    #[error("Pi gateway requires a supported wire protocol in {field}")]
    UnsupportedApi { field: &'static str },
    #[error("Pi gateway requires a non-empty model identifier")]
    EmptyModel,
}

/// The endpoint fields a synthesized provider document is built from. A registered provider needs
/// a model entry; a built-in override does not.
struct EndpointDocument<'a> {
    base_url: &'a str,
    api: Option<&'a str>,
    model: Option<&'a str>,
    reasoning: bool,
    context_window: Option<u32>,
    max_tokens: Option<u32>,
    /// Environment field the document interpolates as `apiKey`, or `None` to omit the field so Pi
    /// resolves its own environment credential. A present `apiKey` that does not resolve suppresses
    /// Pi's environment auth, so it is written only for a field the lane actually supplies.
    api_key_field: Option<&'a str>,
}

/// A caller-owned gateway endpoint: the base URL, the wire protocol the caller named, the model
/// identifier the caller owns, whether the binding asked for reasoning, and any declared limits.
pub(super) struct GatewayDocument<'a> {
    pub(super) base_url: &'a str,
    pub(super) api: &'a str,
    pub(super) model: &'a str,
    pub(super) reasoning: bool,
    pub(super) context_window: Option<u32>,
    pub(super) max_tokens: Option<u32>,
}

/// Builds the provider document. The API key is read from the declared connection through Pi's
/// own `$NAME` interpolation, so the secret never reaches this file.
fn models_document(selection: ProviderOverride, endpoint: EndpointDocument<'_>) -> Value {
    let mut provider = serde_json::Map::new();
    provider.insert("baseUrl".to_owned(), json!(endpoint.base_url));
    if let Some(api) = endpoint.api {
        provider.insert("api".to_owned(), json!(api));
    }
    if let Some(api_key_field) = endpoint.api_key_field {
        provider.insert("apiKey".to_owned(), json!(format!("${api_key_field}")));
    }
    // A registered provider needs one model entry before it resolves any identifier. The entry
    // binds the caller's own identifier, and `reasoning` mirrors the binding's authored effort so
    // Pi does not silently discard it. Context window and output limit are written only when the
    // caller declared them; otherwise Pi owns those defaults rather than Zeroshot maintaining a
    // catalog. Cost stays Pi-owned either way.
    if let Some(model) = endpoint.model {
        let mut entry = serde_json::Map::new();
        entry.insert("id".to_owned(), json!(model));
        entry.insert("name".to_owned(), json!(model));
        entry.insert("api".to_owned(), json!(endpoint.api));
        entry.insert("reasoning".to_owned(), json!(endpoint.reasoning));
        if let Some(context_window) = endpoint.context_window {
            entry.insert("contextWindow".to_owned(), json!(context_window));
        }
        if let Some(max_tokens) = endpoint.max_tokens {
            entry.insert("maxTokens".to_owned(), json!(max_tokens));
        }
        provider.insert("models".to_owned(), json!([entry]));
    }
    json!({ "providers": { selection.provider: Value::Object(provider) } })
}

/// Writes the provider document into the private agent directory for this turn.
///
/// The file is rewritten every turn so a resumed session cannot read a stale endpoint. It holds the
/// caller-owned base URL and an interpolation reference, never the secret itself.
pub(super) fn write_gateway(
    agent_dir: &Path,
    document: GatewayDocument<'_>,
) -> Result<PathBuf, PiProviderDocumentError> {
    validate_api(document.api)?;
    if document.model.trim().is_empty() {
        return Err(PiProviderDocumentError::EmptyModel);
    }
    write_document(
        agent_dir,
        models_document(
            GATEWAY,
            EndpointDocument {
                base_url: document.base_url,
                api: Some(document.api),
                model: Some(document.model),
                reasoning: document.reasoning,
                context_window: document.context_window,
                max_tokens: document.max_tokens,
                api_key_field: Some(GATEWAY.api_key_field),
            },
        ),
    )
}

/// Rejects an unsupported wire protocol before any child process starts. The same allowlist gates
/// the launch-time credential check, so the guarantee does not depend on which layer runs first.
pub(super) fn validate_api(api: &str) -> Result<(), PiProviderDocumentError> {
    if SUPPORTED_APIS.contains(&api) {
        Ok(())
    } else {
        Err(PiProviderDocumentError::UnsupportedApi {
            field: "GATEWAY_API",
        })
    }
}

/// Writes a built-in-provider endpoint override, which keeps that provider's own protocol and its
/// built-in model catalog. `api_key_field` names the credential field the document interpolates,
/// or `None` to omit `apiKey` so Pi resolves the lane's environment credential itself.
pub(super) fn write_endpoint(
    agent_dir: &Path,
    selection: ProviderOverride,
    base_url: &str,
    api_key_field: Option<&str>,
) -> Result<PathBuf, PiProviderDocumentError> {
    write_document(
        agent_dir,
        models_document(
            selection,
            EndpointDocument {
                base_url,
                api: None,
                model: None,
                reasoning: false,
                context_window: None,
                max_tokens: None,
                api_key_field,
            },
        ),
    )
}

fn write_document(agent_dir: &Path, document: Value) -> Result<PathBuf, PiProviderDocumentError> {
    let bytes = serde_json::to_vec_pretty(&document).map_err(PiProviderDocumentError::Serialize)?;
    crate::native_v2_capsule::provider_process::write_provider_file(agent_dir, MODELS_FILE, &bytes)
        .map_err(PiProviderDocumentError::Write)
}

/// Resolves the gateway base URL from the declared connection, so a missing or invalid connection
/// fails before any child process starts.
pub(super) fn gateway_base_url(resolved: &ResolvedEnvironment) -> Result<String, NodeRunnerError> {
    gateway::connection(&declared(resolved)).map(|(base_url, _)| base_url.to_owned())
}

/// Resolves a caller-supplied endpoint override. An absent field means the provider's own endpoint
/// applies and no document is written.
pub(super) fn declared_base_url(resolved: &ResolvedEnvironment, field: &str) -> Option<String> {
    let value = resolved
        .iter()
        .find(|(name, _)| name.as_str() == field)?
        .1
        .trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Reads the caller-owned wire protocol. A missing or unrecognized value is a usage failure rather
/// than a guess, because Pi has no way to report which protocol an opaque identifier needs.
pub(super) fn declared_api(
    resolved: &ResolvedEnvironment,
    field: &str,
) -> Result<String, NodeRunnerError> {
    let value = declared_base_url(resolved, field).ok_or_else(|| {
        NodeRunnerError::DriverDetail(format!(
            "Pi gateway requires {field} to name one of {}",
            SUPPORTED_APIS.join(", ")
        ))
    })?;
    if !SUPPORTED_APIS.contains(&value.as_str()) {
        return Err(NodeRunnerError::DriverDetail(format!(
            "Pi gateway does not support wire protocol {value}; expected one of {}",
            SUPPORTED_APIS.join(", ")
        )));
    }
    Ok(value)
}

fn declared(resolved: &ResolvedEnvironment) -> BTreeMap<String, String> {
    resolved
        .iter()
        .map(|(name, value)| (name.as_str().to_owned(), value.to_owned()))
        .collect()
}

#[cfg(test)]
#[path = "provider_document/tests.rs"]
mod tests;
