#[path = "support/assert_value.rs"]
mod assert_value;

#[path = "support/json_read.rs"]
mod json_read;

use assert_value::AssertValue;
use openengine_cluster_protocol::{
    ActiveExecution, Cursor, ExecutionRef, NodeName, RunAttachEventNotification, RunAttachParams,
    RunForceParams, RunForceResult, RunId, RunLogEventNotification, RunLogsParams, RunMetadata,
    RunSize, RunStatus, RunStatusParams, RunStatusResult, RunTitle, RunWatchEventNotification,
    RunWatchParams, ResolvedSource, SubscriptionId, TerminalResult, TokenCount, TokenUsage,
    UnixTimestampMillis, MAX_SAFE_GENERATION,
};
use serde_json::json;

fn title() -> RunTitle {
    RunTitle::new("Repair checkout flow").assert_value()
}

fn source() -> ResolvedSource {
    serde_json::from_slice(
        br#"{
            "repository": "open-engine/zeroshot",
            "branch": "main",
            "revision": "0123456789abcdef0123456789abcdef01234567"
        }"#,
    )
    .assert_value()
}

fn execution(value: &str) -> ExecutionRef {
    ExecutionRef::new(value).assert_value()
}

fn active(execution_ref: &str, node: &str) -> ActiveExecution {
    ActiveExecution {
        map_indices: Vec::new(),
        execution: execution(execution_ref),
        node: NodeName::new(node).assert_value(),
    }
}

fn running_status() -> RunStatus {
    RunStatus::Running {
        suspend_requested: false,
        graph_result: None,
        active_executions: vec![
            active("opaque-verifier-a", "verify-a"),
            active("opaque-verifier-b", "verify-b"),
        ],
    }
}

#[test]
fn graph_result_round_trips_only_in_unfinished_started_phases() {
    for phase in ["running", "stopping"] {
        for result in [
            json!({"status":"succeeded","output":{"answer":42}}),
            json!({"status":"failed","reason":"worker_failed","context":{"recorded":[],"incomplete":true}}),
        ] {
            let value = json!({"phase":phase,"activeExecutions":[],"graphResult":result});
            let status: RunStatus = serde_json::from_value(value.clone()).assert_value();
            assert_eq!(serde_json::to_value(status).assert_value(), value);
        }
        let legacy = json!({"phase":phase,"activeExecutions":[]});
        let status: RunStatus = serde_json::from_value(legacy.clone()).assert_value();
        assert_eq!(serde_json::to_value(status).assert_value(), legacy);
    }
    for value in [
        json!({"phase":"admitted","graphResult":{"status":"succeeded","output":null}}),
        json!({"phase":"finished","terminalResult":{"status":"succeeded","output":null},
            "graphResult":{"status":"succeeded","output":null}}),
        json!({"phase":"running","activeExecutions":[],"graphResult":{"status":"failed"}}),
    ] {
        assert!(serde_json::from_value::<RunStatus>(value).is_err());
    }
}

#[test]
fn status_exposes_every_parallel_execution_without_private_identity() {
    let mut result = RunStatusResult {
        run_id: RunId::new("run-1"),
        title: title(),
        source: Some(source()),
        size: RunSize::new("medium").assert_value(),
        at_cursor: Cursor::new("v2:7"),
        status: running_status(),
        controller: None,
        workspace_recovery: Default::default(),
    };
    let value = serde_json::to_value(&result).assert_value();

    assert_eq!(
        value,
        json!({
            "runId": "run-1",
            "title": "Repair checkout flow",
            "source": {
                "repository": "open-engine/zeroshot",
                "branch": "main",
                "revision": "0123456789abcdef0123456789abcdef01234567"
            },
            "size": "medium",
            "atCursor": "v2:7",
            "status": {
                "phase": "running",
                "activeExecutions": [
                    { "execution": "opaque-verifier-a", "node": "verify-a" },
                    { "execution": "opaque-verifier-b", "node": "verify-b" }
                ]
            }
        })
    );
    let encoded = serde_json::to_string(&value).assert_value();
    for private_name in ["capsule", "session", "provider", "executionId"] {
        assert!(!encoded.contains(private_name));
    }
    result.workspace_recovery.recoverable = true;
    assert_eq!(
        serde_json::to_value(&result).assert_value()["workspaceRecovery"],
        json!({"recoverable": true})
    );
    assert!(
        serde_json::from_value::<RunStatusResult>(json!({
            "runId": "run-1",
            "title": "Repair checkout flow",
            "source": {
                "repository": "open-engine/zeroshot",
                "branch": "main",
                "revision": "0123456789abcdef0123456789abcdef01234567"
            },
            "size": "medium",
            "atCursor": "v2:7",
            "status": {
                "phase": "running",
                "activeExecutions": [],
                "sessionId": "private"
            }
        }))
        .is_err()
    );
}

#[test]
fn watch_and_logs_use_required_run_id_and_exclusive_resume_cursor() {
    let watch = RunWatchParams {
        run_id: RunId::new("run-1"),
        from_cursor: Some(Cursor::new("v2:7")),
    };
    assert_eq!(
        serde_json::to_value(&watch).assert_value(),
        json!({ "runId": "run-1", "fromCursor": "v2:7" })
    );

    let logs = RunLogsParams {
        run_id: RunId::new("run-1"),
        from_cursor: Some(Cursor::new("v2:7")),
        execution: Some(execution("opaque-verifier-b")),
    };
    assert_eq!(
        serde_json::to_value(&logs).assert_value(),
        json!({
            "runId": "run-1",
            "fromCursor": "v2:7",
            "execution": "opaque-verifier-b"
        })
    );

    assert!(serde_json::from_value::<RunWatchParams>(json!({})).is_err());
    assert!(serde_json::from_value::<RunLogsParams>(json!({ "fromCursor": "v2:7" })).is_err());
}

#[test]
fn token_usage_rejects_counters_that_javascript_cannot_represent_exactly() {
    assert!(TokenCount::new(MAX_SAFE_GENERATION).is_ok());
    assert!(TokenCount::new(MAX_SAFE_GENERATION + 1).is_err());
    assert!(
        serde_json::from_value::<TokenUsage>(json!({
            "inputTokens": MAX_SAFE_GENERATION + 1,
            "outputTokens": 0,
            "complete": true
        }))
        .is_err()
    );
}

#[test]
fn log_timestamps_are_positive_javascript_safe_unix_milliseconds() {
    assert!(UnixTimestampMillis::new(1).is_ok());
    assert!(UnixTimestampMillis::new(MAX_SAFE_GENERATION).is_ok());
    assert!(UnixTimestampMillis::new(0).is_err());
    assert!(UnixTimestampMillis::new(MAX_SAFE_GENERATION + 1).is_err());
    assert!(serde_json::from_value::<UnixTimestampMillis>(json!(0)).is_err());
    assert!(serde_json::from_value::<UnixTimestampMillis>(json!(MAX_SAFE_GENERATION + 1)).is_err());
}

#[test]
fn durable_events_carry_run_and_stable_cursor() {
    let terminal_output = json!({
        "kind": "verification_receipt",
        "summary": "checkout repaired",
        "passed": true
    });
    let watch = RunWatchEventNotification {
        subscription_id: SubscriptionId::new("watch-1"),
        run_id: RunId::new("run-1"),
        title: title(),
        source: Some(source()),
        size: RunSize::new("medium").assert_value(),
        cursor: Cursor::new("v2:8"),
        status: RunStatus::Finished {
            terminal_result: TerminalResult::Succeeded {
                output: terminal_output.clone(),
            },
            metadata: RunMetadata {
                token_usage: Some(TokenUsage {
                    input_tokens: TokenCount::new(17).assert_value(),
                    output_tokens: TokenCount::new(3).assert_value(),
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                    complete: false,
                }),
                delivery_receipt: None,
            },
        },
    };
    let value = serde_json::to_value(&watch).assert_value();
    assert_eq!(json_read::json_at(&value, "/cursor"), &json!("v2:8"));
    assert_eq!(
        json_read::json_at(&value, "/title"),
        &json!("Repair checkout flow")
    );
    assert_eq!(json_read::json_at(&value, "/size"), &json!("medium"));
    assert_eq!(
        json_read::json_at(&value, "/status/terminalResult/output"),
        &terminal_output
    );
    assert_eq!(
        json_read::json_at(&value, "/status/metadata/tokenUsage"),
        &json!({
            "inputTokens": 17,
            "outputTokens": 3,
            "complete": false
        })
    );

    let value = json!({
        "subscriptionId": "logs-1",
        "runId": "run-1",
        "cursor": "v2:9",
        "timestamp": 1725000000123_u64,
        "execution": "opaque-verifier-b",
        "record": {
            "level": "info",
            "target": "agent",
            "message": "historical output"
        }
    });
    let log: RunLogEventNotification = serde_json::from_value(value.clone()).assert_value();
    assert_eq!(serde_json::to_value(log).assert_value(), value);
    assert_eq!(json_read::json_at(&value, "/runId"), &json!("run-1"));
    assert_eq!(json_read::json_at(&value, "/cursor"), &json!("v2:9"));
    assert_eq!(
        json_read::json_at(&value, "/timestamp"),
        &json!(1_725_000_000_123_u64)
    );
    assert_eq!(
        json_read::json_at(&value, "/execution"),
        &json!("opaque-verifier-b")
    );
}

#[test]
fn attach_is_live_read_only_and_explicitly_selects_one_execution() {
    let params = RunAttachParams {
        run_id: RunId::new("run-1"),
        execution: execution("opaque-verifier-b"),
    };
    assert_eq!(
        serde_json::to_value(&params).assert_value(),
        json!({ "runId": "run-1", "execution": "opaque-verifier-b" })
    );
    assert!(
        serde_json::from_value::<RunAttachParams>(json!({
            "runId": "run-1",
            "execution": "opaque-verifier-b",
            "input": "write to the agent"
        }))
        .is_err()
    );

    let value = json!({
        "subscriptionId": "attach-1",
        "runId": "run-1",
        "execution": "opaque-verifier-b",
        "event": { "type": "output", "text": "live output" }
    });
    let event: RunAttachEventNotification = serde_json::from_value(value.clone()).assert_value();
    assert_eq!(serde_json::to_value(event).assert_value(), value);
    assert_eq!(json_read::json_at(&value, "/runId"), &json!("run-1"));
    assert_eq!(
        json_read::json_at(&value, "/execution"),
        &json!("opaque-verifier-b")
    );
    assert!(value.get("cursor").is_none());
}

#[test]
fn force_is_the_only_stop_shape_and_returns_durable_status() {
    let params = RunForceParams {
        run_id: RunId::new("run-1"),
    };
    assert_eq!(
        serde_json::to_value(&params).assert_value(),
        json!({ "runId": "run-1" })
    );
    assert!(
        serde_json::from_value::<RunForceParams>(json!({
            "runId": "run-1",
            "mode": "drain"
        }))
        .is_err()
    );

    let result = RunForceResult {
        run_id: RunId::new("run-1"),
        title: title(),
        source: Some(source()),
        size: RunSize::new("medium").assert_value(),
        at_cursor: Cursor::new("v2:10"),
        status: RunStatus::Stopping {
            graph_result: None,
            active_executions: vec![active("opaque-verifier-b", "verify-b")],
        },
        controller: None,
        workspace_recovery: Default::default(),
    };
    let value = serde_json::to_value(result).assert_value();
    assert_eq!(
        json_read::json_at(&value, "/status/phase"),
        &json!("stopping")
    );
    assert!(value.get("workspaceRecovery").is_none());
}

#[test]
fn every_operation_rejects_unknown_or_missing_public_run_identity() {
    assert!(serde_json::from_value::<RunStatusParams>(json!({})).is_err());
    assert!(
        serde_json::from_value::<RunStatusParams>(json!({
            "runId": "run-1",
            "capsuleId": "private"
        }))
        .is_err()
    );
}

#[test]
fn controller_availability_is_optional_observation_metadata() {
    use openengine_cluster_protocol::{ControllerObservation, ControllerState};
    for state in [ControllerState::Lost, ControllerState::Unknown] {
        let result = RunStatusResult {
            run_id: RunId::new("run-1"),
            title: title(),
            source: None,
            size: RunSize::new("host").assert_value(),
            at_cursor: Cursor::new("v2:7"),
            status: running_status(),
            workspace_recovery: Default::default(),
            controller: Some(ControllerObservation {
                state,
                observed_at: UnixTimestampMillis::new(1_794_470_707_000).assert_value(),
            }),
        };
        let value = serde_json::to_value(&result).assert_value();
        assert_eq!(
            serde_json::from_value::<RunStatusResult>(value.clone()).assert_value(),
            result
        );
        assert_eq!(value["status"]["phase"], "running");
        assert_eq!(value["controller"]["observedAt"], 1_794_470_707_000_u64);
        let mut invalid = value;
        invalid["controller"]["observedAt"] = json!(0);
        assert!(serde_json::from_value::<RunStatusResult>(invalid).is_err());
    }
}

#[test]
fn suspended_statuses_round_trip_without_terminal_result() {
    for value in [
        json!({"phase":"suspended","reason":"command","node":"review","createdAt":1762905600000_u64,"metadata":{}}),
        json!({"phase":"suspended","reason":"wait","node":"cooldown","createdAt":1762905600000_u64,"wait":{"dueAt":1762992000000_u64},"metadata":{}}),
        json!({"phase":"running","activeExecutions":[],"suspendRequested":true}),
    ] {
        let status: RunStatus = serde_json::from_value(value.clone()).assert_value();
        assert_eq!(serde_json::to_value(status).assert_value(), value);
    }
}

#[test]
fn request_suspension_round_trips_with_its_full_contract() {
    let value = json!({"phase":"suspended","reason":"request","node":"approval","createdAt":1000,
        "request":{"id":"request-1","text":"Approve?","signals":{"decision":["approved","rejected"]},"output":{"kind":"record","fields":{}},"expiresAt":2000},"metadata":{}});
    let status: RunStatus = serde_json::from_value(value.clone()).assert_value();
    assert_eq!(serde_json::to_value(status).assert_value(), value);
    let mut missing = value.clone();
    missing.as_object_mut().assert_value().remove("request");
    assert!(serde_json::from_value::<RunStatus>(missing).is_err());
    let mut wrong = value;
    wrong["reason"] = json!("command");
    assert!(serde_json::from_value::<RunStatus>(wrong).is_err());
}
