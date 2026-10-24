//! Tests for `native_v2_pi/provider_document.rs`.
//!
//! Kept outside the production-named source file so production coverage totals exclude them
//! (`AGENTS.md`, `COVERAGE_IGNORE_REGEX` in `.github/workflows/coverage.yml`).
//!
//! These assertions are portable, so unlike the adapter's own test file they are not gated to Unix.

use super::{
    ANTHROPIC, EndpointDocument, GATEWAY, GatewayDocument, MODELS_FILE, OPENAI, SUPPORTED_APIS,
    declared_api, declared_base_url, gateway_base_url, models_document, write_endpoint,
    write_gateway,
};
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_contract::{
    DeclaredConnections, DeclaredEnvironment, EnvironmentVariableName, NodeRuntimeBinding,
};
use crate::native_v2_runner::ResolvedEnvironment;
use std::collections::BTreeMap;

/// The shared `environment_name` fixture is Unix-only; the contract type it wraps is not.
fn environment_name(value: &str) -> EnvironmentVariableName {
    EnvironmentVariableName::new(value).unwrap()
}

/// A resolved environment carrying exactly the supplied values, through the declared-connection
/// path the runner uses.
fn resolved(fields: &[&str], values: &[(&str, &str)]) -> ResolvedEnvironment {
    let names = fields
        .iter()
        .map(|name| environment_name(name))
        .collect::<Vec<_>>();
    let binding = NodeRuntimeBinding::Agent {
        lane: None,
        model: crate::worker_catalog::ModelId::new("provider-model").unwrap(),
        effort: None,
        context_window: None,
        max_tokens: None,
        session_scope: crate::execution::SessionScope::Execution,
        connections: if names.is_empty() {
            DeclaredConnections::empty()
        } else {
            DeclaredConnections::single("provider", DeclaredEnvironment::new(names).unwrap())
                .unwrap()
        },
    };
    let mut map: BTreeMap<EnvironmentVariableName, String> = BTreeMap::new();
    for (name, value) in values {
        map.insert(
            EnvironmentVariableName::new(*name).unwrap(),
            (*value).to_owned(),
        );
    }
    ResolvedEnvironment::exact(&binding, map).unwrap()
}

fn gateway_connection(api: Option<&str>) -> ResolvedEnvironment {
    let mut fields = vec!["GATEWAY_BASE_URL", "GATEWAY_API_KEY"];
    let mut values = vec![
        ("GATEWAY_BASE_URL", "https://gateway.example/api/v1"),
        ("GATEWAY_API_KEY", "sk-gateway"),
    ];
    if let Some(api) = api {
        fields.push("GATEWAY_API");
        values.push(("GATEWAY_API", api));
    }
    resolved(&fields, &values)
}

#[test]
fn the_gateway_registers_its_own_provider_with_the_caller_protocol() {
    // Pi honors `api` on a registered provider and ignores it on a built-in one, so the lane
    // registers `zeroshot-gateway` rather than retargeting `openai`.
    let document = models_document(
        GATEWAY,
        EndpointDocument {
            base_url: "https://gateway.example/api/v1",
            api: Some("openai-completions"),
            model: Some("provider-model"),
            reasoning: true,
            context_window: None,
            max_tokens: None,
            api_key_field: Some(GATEWAY.api_key_field),
        },
    );
    let provider = &document["providers"][GATEWAY.provider];
    assert_eq!(provider["api"], "openai-completions");
    assert_eq!(provider["baseUrl"], "https://gateway.example/api/v1");
    // The key stays in the declared connection, never in the synthesized file.
    assert_eq!(provider["apiKey"], "$GATEWAY_API_KEY");
    // The model entry binds the caller's identifier and only the limits the caller declared.
    // Zeroshot owns no capability defaults and no catalog.
    let models = provider["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], "provider-model");
    assert_eq!(models[0]["api"], "openai-completions");
    assert!(models[0].get("contextWindow").is_none());
    assert!(models[0].get("maxTokens").is_none());
    assert!(models[0].get("cost").is_none());
    // Reasoning mirrors the binding's authored effort so Pi cannot silently discard it.
    assert_eq!(models[0]["reasoning"], true);
}

#[test]
fn caller_declared_model_limits_are_emitted_only_when_supplied() {
    fn entry(context_window: Option<u32>, max_tokens: Option<u32>) -> serde_json::Value {
        models_document(
            GATEWAY,
            EndpointDocument {
                base_url: "https://gateway.example/api/v1",
                api: Some("openai-responses"),
                model: Some("provider-model"),
                reasoning: false,
                context_window,
                max_tokens,
                api_key_field: Some(GATEWAY.api_key_field),
            },
        )["providers"][GATEWAY.provider]["models"][0]
            .clone()
    }

    let both = entry(Some(1_000_000), Some(32_000));
    assert_eq!(both["contextWindow"], 1_000_000);
    assert_eq!(both["maxTokens"], 32_000);

    // Each field is independent; an omitted field stays absent rather than being defaulted.
    let context_only = entry(Some(1_000_000), None);
    assert_eq!(context_only["contextWindow"], 1_000_000);
    assert!(context_only.get("maxTokens").is_none());
    let output_only = entry(None, Some(32_000));
    assert!(output_only.get("contextWindow").is_none());
    assert_eq!(output_only["maxTokens"], 32_000);
}

#[test]
fn the_written_provider_file_holds_no_secret() {
    // The only credential reference is an interpolation of a constant field name, so the
    // caller's key never reaches disk even though the base URL does.
    let directory = TestDirectory::new("pi-gateway-secret");
    let agent_dir = directory.child("agent");
    write_gateway(
        &agent_dir,
        GatewayDocument {
            base_url: "https://gateway.example/api/v1",
            api: "openai-responses",
            model: "provider-model",
            reasoning: false,
            context_window: None,
            max_tokens: None,
        },
    )
    .unwrap();
    let written = std::fs::read_to_string(directory.child("agent/models.json")).unwrap();
    assert!(written.contains("$GATEWAY_API_KEY"));
    assert!(!written.contains("sk-gateway"));
}

#[test]
fn every_supported_protocol_is_accepted_and_nothing_else_is() {
    for api in SUPPORTED_APIS {
        assert_eq!(
            declared_api(&gateway_connection(Some(api)), "GATEWAY_API").unwrap(),
            api
        );
    }
    // A missing or unrecognized protocol is a usage failure, never a guess.
    assert!(declared_api(&gateway_connection(None), "GATEWAY_API").is_err());
    assert!(declared_api(&gateway_connection(Some("grpc")), "GATEWAY_API").is_err());
    assert!(declared_api(&gateway_connection(Some("  ")), "GATEWAY_API").is_err());
}

#[test]
fn the_gateway_document_is_written_and_validated() {
    let directory = TestDirectory::new("pi-gateway-doc");
    let agent_dir = directory.child("agent");
    let path = write_gateway(
        &agent_dir,
        GatewayDocument {
            base_url: "https://gateway.example/api/v1",
            api: "anthropic-messages",
            model: "provider-model",
            reasoning: true,
            context_window: None,
            max_tokens: None,
        },
    )
    .unwrap();
    assert_eq!(path, agent_dir.join(MODELS_FILE));
    assert!(
        std::fs::read_to_string(directory.child("agent/models.json"))
            .unwrap()
            .contains("anthropic-messages")
    );

    // An unsupported protocol and an empty identifier fail before any child process starts.
    assert!(
        write_gateway(
            &agent_dir,
            GatewayDocument {
                base_url: "https://x.invalid",
                api: "grpc",
                model: "m",
                reasoning: false,
                context_window: None,
                max_tokens: None,
            },
        )
        .is_err()
    );
    assert!(
        write_gateway(
            &agent_dir,
            GatewayDocument {
                base_url: "https://x.invalid",
                api: "openai-responses",
                model: "  ",
                reasoning: false,
                context_window: None,
                max_tokens: None,
            },
        )
        .is_err()
    );
}

#[test]
fn the_written_provider_file_is_owner_only() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let directory = TestDirectory::new("pi-gateway-mode");
        let agent_dir = directory.child("agent");
        let path = write_gateway(
            &agent_dir,
            GatewayDocument {
                base_url: "https://gateway.example/api/v1",
                api: "openai-responses",
                model: "provider-model",
                reasoning: false,
                context_window: None,
                max_tokens: None,
            },
        )
        .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the provider file must be owner-only");
    }
}

#[test]
fn the_anthropic_document_keeps_pis_own_messages_protocol() {
    // This is the escape hatch Claude Code offers through `ANTHROPIC_BASE_URL`: retarget the
    // endpoint without choosing a wire protocol, because overriding one is ignored anyway.
    let document = models_document(
        ANTHROPIC,
        EndpointDocument {
            base_url: "https://messages.example",
            api: None,
            model: None,
            reasoning: false,
            context_window: None,
            max_tokens: None,
            api_key_field: Some(ANTHROPIC.api_key_field),
        },
    );
    let provider = &document["providers"]["anthropic"];
    assert_eq!(provider["baseUrl"], "https://messages.example");
    assert_eq!(provider["apiKey"], "$ANTHROPIC_API_KEY");
    assert!(
        provider.get("api").is_none(),
        "the Anthropic Messages protocol must stay the built-in one"
    );
    assert!(provider.get("models").is_none());
}

#[test]
fn the_anthropic_endpoint_omits_api_key_when_only_a_token_is_available() {
    // Pi suppresses its own environment auth when a `models.json` `apiKey` does not resolve, so
    // a caller-owned endpoint that carries `ANTHROPIC_AUTH_TOKEN` or `ANTHROPIC_OAUTH_TOKEN`
    // must omit `apiKey` and let Pi resolve that token from the environment itself.
    let document = models_document(
        ANTHROPIC,
        EndpointDocument {
            base_url: "https://messages.example",
            api: None,
            model: None,
            reasoning: false,
            context_window: None,
            max_tokens: None,
            api_key_field: None,
        },
    );
    let provider = &document["providers"]["anthropic"];
    assert_eq!(provider["baseUrl"], "https://messages.example");
    assert!(
        provider.get("apiKey").is_none(),
        "an unavailable key field must not suppress Pi's environment auth"
    );
    assert!(provider.get("api").is_none());
}

#[test]
fn the_openai_document_keeps_pis_own_responses_protocol() {
    // The OpenAI lane retargets the endpoint and keeps the built-in protocol, so an endpoint
    // for this lane must speak Responses. The gateway lane is the one that can name a protocol.
    let document = models_document(
        OPENAI,
        EndpointDocument {
            base_url: "https://openai-proxy.example/v1",
            api: None,
            model: None,
            reasoning: false,
            context_window: None,
            max_tokens: None,
            api_key_field: Some(OPENAI.api_key_field),
        },
    );
    let provider = &document["providers"]["openai"];
    assert_eq!(provider["baseUrl"], "https://openai-proxy.example/v1");
    assert_eq!(provider["apiKey"], "$OPENAI_API_KEY");
    assert!(provider.get("api").is_none());
    assert!(provider.get("models").is_none());
}

#[test]
fn the_provider_file_lands_in_the_agent_directory() {
    assert_eq!(MODELS_FILE, "models.json");
    let directory = TestDirectory::new("pi-anthropic-file");
    let agent_dir = directory.child("agent");
    let path = write_endpoint(
        &agent_dir,
        ANTHROPIC,
        "https://messages.example",
        Some(ANTHROPIC.api_key_field),
    )
    .unwrap();
    assert_eq!(path, agent_dir.join("models.json"));
}

#[test]
fn a_declared_endpoint_override_is_read_only_when_present_and_nonempty() {
    assert_eq!(
        declared_base_url(
            &resolved(
                &["ANTHROPIC_BASE_URL"],
                &[("ANTHROPIC_BASE_URL", "https://messages.example")]
            ),
            "ANTHROPIC_BASE_URL"
        )
        .as_deref(),
        Some("https://messages.example")
    );
    // An absent field means the provider's own endpoint applies and nothing is written.
    assert!(declared_base_url(&resolved(&[], &[]), "ANTHROPIC_BASE_URL").is_none());
    assert!(
        declared_base_url(
            &resolved(&["ANTHROPIC_BASE_URL"], &[("ANTHROPIC_BASE_URL", "   ")]),
            "ANTHROPIC_BASE_URL"
        )
        .is_none()
    );
}

#[test]
fn a_complete_gateway_connection_resolves_its_base_url() {
    assert_eq!(
        gateway_base_url(&gateway_connection(Some("openai-responses"))).unwrap(),
        "https://gateway.example/api/v1"
    );
}

#[test]
fn an_incomplete_gateway_connection_fails_before_launch() {
    for (fields, values) in [
        (
            vec!["GATEWAY_BASE_URL"],
            vec![("GATEWAY_BASE_URL", "https://gateway.example")],
        ),
        (
            vec!["GATEWAY_API_KEY"],
            vec![("GATEWAY_API_KEY", "sk-gateway")],
        ),
        (
            vec!["GATEWAY_BASE_URL", "GATEWAY_API_KEY"],
            vec![("GATEWAY_BASE_URL", ""), ("GATEWAY_API_KEY", "k")],
        ),
        (vec![], vec![]),
    ] {
        assert!(
            gateway_base_url(&resolved(&fields, &values)).is_err(),
            "incomplete gateway connection must fail before launch"
        );
    }
}
