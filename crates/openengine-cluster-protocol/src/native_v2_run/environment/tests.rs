use super::*;
use crate::RuntimePlan;
use serde_json::json;

#[test]
fn environment_rejects_reserved_names_oversized_scripts_and_ambiguous_credentials() {
    for name in ["PATH", "HOME", "ZEROSHOT_TOOLS", "CODEX_HOME", "TMPDIR"] {
        let environment: RuntimeEnvironment =
            serde_json::from_value(json!({"variables": {name: "override"}})).unwrap();
        assert!(environment.validate().is_err());
    }
    let oversized = RuntimeEnvironment {
        setup: Some("x".repeat(MAX_RUNTIME_SCRIPT_BYTES + 1)),
        ..Default::default()
    };
    assert!(oversized.validate().is_err());
    let environment: RuntimeEnvironment = serde_json::from_value(json!({
        "variables": {"REGISTRY_TOKEN":"public"}, "connections": {"registry":["REGISTRY_TOKEN"]}
    }))
    .unwrap();
    assert!(environment.validate().is_err());
    let environment: RuntimeEnvironment = serde_json::from_value(json!({
        "startup":"echo ready", "variables":{"NODE_ENV":"test"}, "connections":{"registry":["REGISTRY_TOKEN"]}
    })).unwrap();
    assert!(environment.validate().is_ok());
}

#[test]
fn preparation_connections_join_run_requirements_without_changing_profiles() {
    let mut value = json!({"harness":"codex", "provider":"openai", "size":"small", "nodes":{}});
    let runtime: RuntimePlan = serde_json::from_value(value.clone()).unwrap();
    let definition: RuntimeEnvironment =
        serde_json::from_value(json!({"connections":{"registry":["REGISTRY_TOKEN"]}})).unwrap();
    assert_eq!(
        crate::run_connection_requirements(&runtime, Some(&definition)).len(),
        1
    );
    assert!(runtime.connection_requirements().is_empty());
    for unsupported in [
        json!({"id":"saved"}),
        serde_json::to_value(definition).unwrap(),
    ] {
        value["environment"] = unsupported;
        assert!(serde_json::from_value::<RuntimePlan>(value.clone()).is_err());
    }
}

#[test]
fn preparation_failures_cannot_be_authored_as_graph_outcomes() {
    for reason in [
        "environment_setup_failed",
        "environment_startup_failed",
        "environment_preparation_timeout",
    ] {
        assert!(crate::FailReason::new(crate::EnumLabel::new(reason).unwrap()).is_err());
    }
}
