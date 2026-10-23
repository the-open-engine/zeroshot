use super::*;
use serde_json::json;

#[test]
fn memory_is_opt_out_with_independent_repository_and_graph_controls() {
    let config: ActiveMemoryConfig = serde_json::from_value(json!({})).expect("valid default");
    assert!(config.repository && config.graph);
    assert_eq!(config.budget_bytes, 2048);
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
    for budget in [0, 16_385, u32::MAX] {
        assert!(
            serde_json::from_value::<ActiveMemoryConfig>(json!({"budgetBytes":budget})).is_err()
        );
    }
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
