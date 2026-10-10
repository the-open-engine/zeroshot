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

fn mixed_runtime(worker: Value, reviewer: Value, router: Value) -> RuntimePlan {
    serde_json::from_value(json!({
        "harness": "codex",
        "provider": "openai",
        "size": "medium",
        "nodes": {
            "worker": {
                "kind": "agent",
                "model": "provider-owned-model",
                "connections": worker
            },
            "reviewer": {
                "kind": "agent",
                "lane": {"harness": "claude", "provider": "anthropic"},
                "model": "provider-owned-model",
                "connections": reviewer
            },
            "router": {
                "kind": "agent",
                "lane": {"harness": "codex", "provider": "openrouter"},
                "model": "provider-owned-model",
                "connections": router
            },
            "deliver": {
                "kind": "git_delivery",
                "connections": {"github": ["GH_TOKEN"]}
            }
        }
    }))
    .assert_value()
}

fn node_connections(runtime: &RuntimePlan) -> Value {
    Value::Object(
        runtime
            .nodes()
            .iter()
            .map(|(name, binding)| {
                (
                    name.as_str().to_owned(),
                    serde_json::to_value(binding.declared_connections()).assert_value(),
                )
            })
            .collect(),
    )
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

#[test]
fn mixed_lanes_materialize_each_node_from_its_own_effective_lane() {
    let mut local = mixed_runtime(json!({}), json!({}), json!({}));
    materialize_provider_access(&mut local, ProviderAccessPlacement::Local).assert_value();
    assert_eq!(
        node_connections(&local),
        json!({
            "deliver":{"github":["GH_TOKEN"]},
            "reviewer":{},
            "router":{"openrouter":["OPENROUTER_API_KEY"]},
            "worker":{}
        })
    );
    assert_eq!(
        requirements(&local),
        json!({"github":["GH_TOKEN"],"openrouter":["OPENROUTER_API_KEY"]})
    );

    let mut contained = mixed_runtime(json!({}), json!({}), json!({}));
    materialize_provider_access(&mut contained, ProviderAccessPlacement::Contained).assert_value();
    assert_eq!(
        node_connections(&contained),
        json!({
            "deliver":{"github":["GH_TOKEN"]},
            "reviewer":{"anthropic":["ANTHROPIC_API_KEY"]},
            "router":{"openrouter":["OPENROUTER_API_KEY"]},
            "worker":{"openai":["OPENAI_API_KEY"]}
        })
    );
    assert_eq!(
        requirements(&contained),
        json!({
            "anthropic":["ANTHROPIC_API_KEY"],
            "github":["GH_TOKEN"],
            "openai":["OPENAI_API_KEY"],
            "openrouter":["OPENROUTER_API_KEY"]
        })
    );
}

#[test]
fn authored_access_wins_only_when_it_fits_the_node_lane() {
    let mut runtime = mixed_runtime(
        json!({"internal":["CODEX_API_KEY"]}),
        json!({"internal":["OPENAI_API_KEY"]}),
        json!({"internal":["OPENROUTER_API_KEY"]}),
    );
    materialize_provider_access(&mut runtime, ProviderAccessPlacement::Contained).assert_value();
    assert_eq!(
        node_connections(&runtime),
        json!({
            "deliver":{"github":["GH_TOKEN"]},
            "reviewer":{"anthropic":["ANTHROPIC_API_KEY"],"internal":["OPENAI_API_KEY"]},
            "router":{"internal":["OPENROUTER_API_KEY"]},
            "worker":{"internal":["CODEX_API_KEY"]}
        })
    );
    assert_eq!(
        requirements(&runtime),
        json!({
            "anthropic":["ANTHROPIC_API_KEY"],
            "github":["GH_TOKEN"],
            "internal":["CODEX_API_KEY","OPENAI_API_KEY","OPENROUTER_API_KEY"]
        })
    );
}
