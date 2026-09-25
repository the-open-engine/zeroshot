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
    let execution = ExecutionId::try_from(7).assert_value();
    assert_eq!(execution.get(), 7);
    assert_eq!(execution.to_string(), "7");
    assert_eq!(serde_json::to_value(execution).assert_value(), json!(7));
    assert_eq!(
        serde_json::from_value::<ExecutionId>(json!(7)).assert_value(),
        execution
    );
    assert!(serde_json::from_value::<ExecutionId>(json!(0)).is_err());
    assert!(serde_json::from_value::<ExecutionId>(json!("7")).is_err());
}

#[test]
fn submission_intent_preserves_caller_input_but_not_resolved_source() {
    let submission: RunSubmission = serde_json::from_value(canonical_submission()).assert_value();
    let intent = RunSubmissionIntent::from(&submission);

    assert_eq!(intent.title, submission.title);
    assert_eq!(intent.graph, submission.graph);
    assert_eq!(intent.initial_input, submission.initial_input);
    assert_eq!(intent.runtime, submission.runtime);
    assert_eq!(intent.submission_key, submission.submission_key);
    assert_eq!(intent.branch, None);
}

#[test]
fn token_usage_parser_accepts_complete_counts_and_rejects_partial_or_unsafe_values() {
    assert_eq!(parse_token_usage_delta(None, None, None), None);
    let usage = json!({
        "input_tokens": 11,
        "output_tokens": 7,
        "cache_read": 5,
        "cache_write": 3
    });
    let parsed = parse_token_usage_delta(Some(&usage), Some("cache_read"), Some("cache_write"))
        .assert_value();
    assert_eq!(parsed.input_tokens.get(), 11);
    assert_eq!(parsed.output_tokens.get(), 7);
    assert_eq!(parsed.cache_read_input_tokens.assert_value().get(), 5);
    assert_eq!(parsed.cache_creation_input_tokens.assert_value().get(), 3);

    let required_only = json!({"input_tokens":2,"output_tokens":1});
    let parsed =
        parse_token_usage_delta(Some(&required_only), None, Some("missing")).assert_value();
    assert_eq!(parsed.cache_read_input_tokens, None);
    assert_eq!(parsed.cache_creation_input_tokens, None);

    for malformed in [
        Value::Null,
        json!({"input_tokens":1}),
        json!({"input_tokens":-1,"output_tokens":1}),
        json!({"input_tokens":1,"output_tokens":1,"cache_read":"many"}),
        json!({"input_tokens":1,"output_tokens":1,"cache_write":"many"}),
    ] {
        assert_eq!(
            parse_token_usage_delta(Some(&malformed), Some("cache_read"), Some("cache_write")),
            None
        );
    }
}

#[test]
fn run_environment_is_separate_from_agent_runtime_and_preserves_explicit_empty() {
    let plain: RunSubmission = serde_json::from_value(canonical_submission()).assert_value();
    assert!(plain.environment.is_none());
    let mut with_environment = canonical_submission();
    with_environment["environment"] = json!({});
    let empty: RunSubmission = serde_json::from_value(with_environment.clone()).assert_value();
    assert_eq!(
        serde_json::to_value(empty).assert_value()["environment"],
        json!({})
    );
    with_environment["environment"] = json!({
        "startup":"npm ci", "variables":{"CI":"true"},
        "connections":{"registry":["NPM_TOKEN"]}
    });
    let request: RunSubmission = serde_json::from_value(with_environment).assert_value();
    assert_eq!(request.runtime, plain.runtime);
    assert!(
        request
            .connection_requirements()
            .contains_key(&ConnectionKey::new("registry").assert_value())
    );
    let intent = RunSubmissionIntent::from(&request);
    assert_eq!(intent.environment, request.environment);
    assert!(
        intent
            .connection_requirements()
            .contains_key(&ConnectionKey::new("registry").assert_value())
    );
    let mut invalid = canonical_submission();
    invalid["runtime"]["environment"] = json!({"startup":"npm ci"});
    assert!(serde_json::from_value::<RunSubmission>(invalid).is_err());
}
