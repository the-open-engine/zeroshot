use openengine_cluster_testkit::assertions::AssertValue;
use super::*;
use serde_json::{json, Value};

fn canonical_submission() -> Value {
    json!({
        "title": "Repair checkout flow",
        "graph": {
            "profile": "openengine.graph.full/v1",
            "initialInput": { "kind": "null" },
            "policy": { "policy": "policy.native-v2@1", "default": "deny" },
            "root": {
                "kind": "seq",
                "name": "run",
                "state": { "kind": "null" },
                "children": [
                    {
                        "kind": "step",
                        "name": "worker",
                        "worker": "agent.worker@1",
                        "instructions": "Implement the requested change.",
                        "input": { "kind": "null" },
                        "output": { "kind": "null" },
                        "inputBindings": [],
                        "writeBindings": [],
                        "timeoutMs": 60000,
                        "attempts": 1
                    },
                    {
                        "kind": "succeed",
                        "name": "done",
                        "output": { "kind": "null" },
                        "bindings": []
                    }
                ],
                "promotedStatePaths": []
            }
        },
        "initialInput": null,
        "runtime": {
            "harness": "codex",
            "provider": "openai",
            "size": "medium",
            "nodes": {
                "worker": {
                    "kind": "agent",
                    "model": "gpt-5.6",
                    "effort": "max",
                    "connections": {
                        "github": ["GH_TOKEN"],
                        "openai": ["OPENAI_API_KEY"]
                    }
                }
            }
        },
        "source": {
            "repository": "open-engine/zeroshot",
            "branch": "main",
            "revision": "0123456789abcdef0123456789abcdef01234567"
        },
        "submissionKey": "submission-1"
    })
}

#[test]
fn canonical_submission_round_trips_without_changing_graph_spec() {
    let expected = canonical_submission();
    let submission: RunSubmission =
        serde_json::from_value(expected.clone()).assert_value_with("canonical fixture must decode");

    let session_scope = submission
        .runtime
        .nodes()
        .get(&NodeName::new("worker").assert_value())
        .and_then(|binding| match binding {
            NodeRuntimeBinding::Agent { session_scope, .. } => Some(session_scope),
            NodeRuntimeBinding::GitDelivery { .. } => None,
        })
        .assert_value_with("worker must be an agent binding");
    assert_eq!(*session_scope, SessionScope::Execution);
    assert_eq!(serde_json::to_value(submission).assert_value(), expected);
}

#[test]
fn submission_serialization_is_idempotent() {
    let submission: RunSubmission = serde_json::from_value(canonical_submission()).assert_value();
    let first = serde_json::to_vec(&submission).assert_value();
    let decoded: RunSubmission = serde_json::from_slice(&first).assert_value();
    let second = serde_json::to_vec(&decoded).assert_value();

    assert_eq!(first, second);
    assert_eq!(decoded, submission);
}

#[test]
fn unsupported_harness_provider_pair_is_rejected_by_shape() {
    let mut fixture = canonical_submission();
    *fixture
        .pointer_mut("/runtime/provider")
        .assert_value_with("runtime provider exists") = json!("anthropic");

    assert!(serde_json::from_value::<RunSubmission>(fixture).is_err());
}

#[test]
fn claude_openrouter_lane_round_trips() {
    let mut expected = canonical_submission();
    *expected.pointer_mut("/runtime/harness").assert_value() = json!("claude");
    *expected.pointer_mut("/runtime/provider").assert_value() = json!("openrouter");
    *expected
        .pointer_mut("/runtime/nodes/worker/model")
        .assert_value() = json!("claude-sonnet-5");

    let submission: RunSubmission = serde_json::from_value(expected.clone()).assert_value();
    assert_eq!(serde_json::to_value(submission).assert_value(), expected);
}

#[test]
fn environment_values_and_graph_runtime_fields_are_rejected() {
    let mut secret_fixture = canonical_submission();
    *secret_fixture
        .pointer_mut("/runtime/nodes/worker/connections/openai")
        .assert_value() = json!({ "OPENAI_API_KEY": "secret" });
    assert!(serde_json::from_value::<RunSubmission>(secret_fixture).is_err());

    let mut graph_fixture = canonical_submission();
    graph_fixture
        .pointer_mut("/graph/root/children/0")
        .assert_value()
        .as_object_mut()
        .assert_value()
        .insert("model".to_owned(), json!("gpt-5.6"));
    assert!(serde_json::from_value::<RunSubmission>(graph_fixture).is_err());
}

#[test]
fn title_source_and_one_run_size_are_required_and_bounded() {
    for (pointer, owner_pointer, field) in [
        ("/title", "", "title"),
        ("/source", "", "source"),
        ("/runtime/size", "/runtime", "size"),
    ] {
        let mut missing = canonical_submission();
        missing
            .pointer_mut(owner_pointer)
            .assert_value()
            .as_object_mut()
            .assert_value()
            .remove(field);
        assert!(
            serde_json::from_value::<RunSubmission>(missing).is_err(),
            "{pointer} must be required"
        );
    }

    let mut invalid_title = canonical_submission();
    *invalid_title.pointer_mut("/title").assert_value() = json!("");
    assert!(serde_json::from_value::<RunSubmission>(invalid_title).is_err());

    let mut invalid_size = canonical_submission();
    *invalid_size.pointer_mut("/runtime/size").assert_value() = json!("xlarge");
    assert!(serde_json::from_value::<RunSubmission>(invalid_size).is_err());

    for (pointer, value) in [
        ("/source/repository", json!("not-a-repository")),
        ("/source/branch", json!("bad..branch")),
        ("/source/revision", json!("not-an-exact-revision")),
    ] {
        let mut invalid_source = canonical_submission();
        *invalid_source.pointer_mut(pointer).assert_value() = value;
        assert!(
            serde_json::from_value::<RunSubmission>(invalid_source).is_err(),
            "{pointer} must be validated"
        );
    }
}

#[test]
fn declared_environment_rejects_duplicates_and_per_node_overflow() {
    let mut duplicate = canonical_submission();
    *duplicate
        .pointer_mut("/runtime/nodes/worker/connections/openai")
        .assert_value() = json!(["OPENAI_API_KEY", "OPENAI_API_KEY"]);
    assert!(serde_json::from_value::<RunSubmission>(duplicate).is_err());

    let mut overflow = canonical_submission();
    *overflow
        .pointer_mut("/runtime/nodes/worker/connections/openai")
        .assert_value() = Value::Array(
        (0..=MAX_DECLARED_ENVIRONMENT_NAMES)
            .map(|index| json!(format!("ENV_{index}")))
            .collect(),
    );
    assert!(serde_json::from_value::<RunSubmission>(overflow).is_err());
}

#[test]
fn environment_names_and_execution_identities_are_bounded() {
    assert!(EnvironmentVariableName::new("GH_TOKEN").is_ok());
    assert!(EnvironmentVariableName::new("GH-TOKEN").is_err());
    assert!(EnvironmentVariableName::new("1TOKEN").is_err());
    assert!(NodeInstanceId::new(0).is_err());
    assert!(ExecutionId::new(0).is_err());
    assert_eq!(ExecutionId::new(7).assert_value().get(), 7);
}
