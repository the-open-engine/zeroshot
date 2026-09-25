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
fn runtime_hook_connections_join_requirements_without_mutating_node_bindings() {
    let runtime: RuntimePlan = serde_json::from_value(json!({
        "harness":"codex", "provider":"openai", "size":"small", "nodes":{},
        "environment":{"connections":{"registry":["REGISTRY_TOKEN"]}}
    }))
    .unwrap();
    assert_eq!(runtime.connection_requirements().len(), 1);
    assert!(runtime.nodes().is_empty());
    let serialized = serde_json::to_value(runtime).unwrap();
    assert_eq!(
        serialized["environment"]["connections"]["registry"][0],
        "REGISTRY_TOKEN"
    );
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

#[test]
fn authored_environments_are_references_and_execution_environments_are_definitions() {
    let authored = json!({
        "harness":"codex", "provider":"openai", "size":"small",
        "nodes":{}, "environment":{"id":"stable-id"}
    });
    let profile: crate::ProfileRuntimePlan = serde_json::from_value(authored.clone()).unwrap();
    assert!(serde_json::from_value::<RuntimePlan>(authored).is_err());
    let resolved = profile.map_environment(|_| {
        Some(RuntimeEnvironment {
            startup: Some("echo ready".into()),
            ..Default::default()
        })
    });
    let execution = serde_json::to_value(resolved).unwrap();
    assert_eq!(execution["environment"]["startup"], "echo ready");
    assert!(serde_json::from_value::<crate::ProfileRuntimePlan>(execution).is_err());
}

#[test]
fn resource_mutations_require_explicit_cas_and_bounded_identities() {
    let request = json!({"name":"shared", "definition":{}});
    assert!(serde_json::from_value::<RuntimeEnvironmentSaveRequest>(request.clone()).is_err());
    let mut create = request;
    create["expectedRevision"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<RuntimeEnvironmentSaveRequest>(create).is_ok());
    for value in ["", "bad id", "bad\nrevision"] {
        assert!(EnvironmentId::new(value).is_err());
        assert!(EnvironmentRevision::new(value).is_err());
    }
    assert!(EnvironmentId::new("x".repeat(129)).is_err());
}
