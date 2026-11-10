use super::*;
use serde_json::json;

#[test]
fn memory_is_opt_out_with_independent_repository_and_graph_controls() {
    let config: ActiveMemoryConfig = serde_json::from_value(json!({})).expect("valid default");
    assert!(config.repository && config.graph);
    assert_eq!(config.node_injection_tokens, 16_000);
    assert_eq!(config.hook_injection_tokens, 8_000);
    assert_eq!(GlobalAgentsConfig::default().postmortem.timeout_ms, 600_000);
    let config: ActiveMemoryConfig =
        serde_json::from_value(json!({"graph": false})).expect("valid opt out");
    assert!(config.repository && !config.graph);
    assert!(config.enabled());
    assert!(
        !ActiveMemoryConfig {
            repository: false,
            graph: false,
            ..Default::default()
        }
        .enabled()
    );
}

#[test]
fn memory_settings_reject_unbounded_budgets_and_unknown_global_agents() {
    for field in ["nodeInjectionTokens", "hookInjectionTokens"] {
        for limit in [0, 64_001, u32::MAX] {
            let mut value = json!({});
            value[field] = json!(limit);
            assert!(serde_json::from_value::<ActiveMemoryConfig>(value).is_err());
        }
    }
    assert!(serde_json::from_value::<ActiveMemoryConfig>(json!({"budgetBytes":512})).is_err());
    assert!(serde_json::from_value::<GlobalAgentsConfig>(json!({"custom":{}})).is_err());
    for timeout in [0, 600_001, u32::MAX] {
        assert!(
            serde_json::from_value::<GlobalAgentsConfig>(
                json!({"postmortem":{"timeoutMs":timeout}})
            )
            .is_err()
        );
    }
}

#[test]
fn postmortem_inherits_a_named_agent_without_accepting_provider_credentials() {
    let config: GlobalAgentsConfig =
        serde_json::from_value(json!({"postmortem":{"sourceNode":"worker","enabled":false}}))
            .expect("valid built in");
    assert_eq!(
        config.postmortem.source_node.as_ref().map(NodeName::as_str),
        Some("worker")
    );
    assert!(!config.postmortem.enabled);
    assert!(
        serde_json::from_value::<GlobalAgentsConfig>(json!({"postmortem":{"model":"invented"}}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<GlobalAgentsConfig>(json!({"postmortem":{"apiKey":"secret"}}))
            .is_err()
    );
}

#[test]
fn injection_token_limits_are_independent_and_round_trip_at_the_safety_bound() {
    let config: ActiveMemoryConfig = serde_json::from_value(json!({
        "nodeInjectionTokens":64_000, "hookInjectionTokens":1
    }))
    .expect("bounded independent limits");
    assert!(config.validate().is_ok());
    let encoded = serde_json::to_value(&config).expect("serializable config");
    assert_eq!(encoded["nodeInjectionTokens"], 64_000);
    assert_eq!(encoded["hookInjectionTokens"], 1);
    assert!(encoded.get("budgetBytes").is_none());
    let invalid = ActiveMemoryConfig {
        node_injection_tokens: 0,
        ..config.clone()
    };
    assert!(invalid.validate().is_err());
    let invalid = ActiveMemoryConfig {
        hook_injection_tokens: 64_001,
        ..config
    };
    assert!(invalid.validate().is_err());
}
