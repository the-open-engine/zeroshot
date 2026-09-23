use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;

fn runtime(harness: &str, provider: &str, connections: Value) -> RuntimePlan {
    serde_json::from_value(json!({
        "harness": harness,
        "provider": provider,
        "size": "medium",
        "nodes": {
            "worker": {
                "kind": "agent",
                "model": "provider-owned-model",
                "connections": connections
            },
            "deliver": {
                "kind": "git_delivery",
                "connections": {"github": ["GH_TOKEN"]}
            }
        }
    }))
    .assert_value()
}

fn requirements(runtime: &RuntimePlan) -> Value {
    serde_json::to_value(runtime.connection_requirements()).assert_value()
}

#[test]
fn native_local_lanes_do_not_invent_provider_connections() {
    for (harness, provider) in [
        ("codex", "openai"),
        ("claude", "anthropic"),
        ("copilot", "github"),
    ] {
        let mut runtime = runtime(harness, provider, json!({}));
        materialize_provider_access(&mut runtime, ProviderAccessPlacement::Local).assert_value();
        assert_eq!(requirements(&runtime), json!({"github":["GH_TOKEN"]}));
    }
}

#[test]
fn contained_and_non_native_lanes_receive_canonical_fallbacks() {
    for (harness, provider, placement, expected) in [
        (
            "codex",
            "openai",
            ProviderAccessPlacement::Contained,
            json!({"openai":["OPENAI_API_KEY"]}),
        ),
        (
            "claude",
            "anthropic",
            ProviderAccessPlacement::Contained,
            json!({"anthropic":["ANTHROPIC_API_KEY"]}),
        ),
        (
            "copilot",
            "github",
            ProviderAccessPlacement::Contained,
            json!({"github":["COPILOT_GITHUB_TOKEN"]}),
        ),
        (
            "codex",
            "openrouter",
            ProviderAccessPlacement::Local,
            json!({"openrouter":["OPENROUTER_API_KEY"]}),
        ),
        (
            "claude",
            "bedrock",
            ProviderAccessPlacement::Local,
            json!({"bedrock":["AWS_BEARER_TOKEN_BEDROCK","AWS_REGION"]}),
        ),
    ] {
        let mut runtime = runtime(harness, provider, json!({}));
        materialize_provider_access(&mut runtime, placement).assert_value();
        let mut expected = expected.as_object().assert_value().clone();
        let github = expected
            .entry("github".to_owned())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .assert_value();
        github.push(json!("GH_TOKEN"));
        github.sort_by_key(ToString::to_string);
        assert_eq!(requirements(&runtime), Value::Object(expected));
    }
}

#[test]
fn authored_access_wins_and_partial_connections_receive_only_missing_fields() {
    let mut codex = runtime("codex", "openai", json!({"internal":["CODEX_API_KEY"]}));
    materialize_provider_access(&mut codex, ProviderAccessPlacement::Contained).assert_value();
    assert_eq!(
        requirements(&codex),
        json!({"github":["GH_TOKEN"],"internal":["CODEX_API_KEY"]})
    );

    let mut gateway = runtime(
        "claude",
        "gateway",
        json!({"internal":["GATEWAY_BASE_URL"]}),
    );
    materialize_provider_access(&mut gateway, ProviderAccessPlacement::Contained).assert_value();
    materialize_provider_access(&mut gateway, ProviderAccessPlacement::Contained).assert_value();
    assert_eq!(
        requirements(&gateway),
        json!({
            "gateway":["GATEWAY_API_KEY"],
            "github":["GH_TOKEN"],
            "internal":["GATEWAY_BASE_URL"]
        })
    );
}
