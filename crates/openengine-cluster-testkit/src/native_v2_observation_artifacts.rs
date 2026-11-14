//! Deterministic schemas and examples for the additive native-v2 observation contract.

use crate::fixture::*;

use openengine_cluster_protocol::{
    ActiveExecution, AgentAttachEvent, BoundedAssistantOutput, BoundedLogMessage, BoundedLogTarget,
    Cursor, ExecutionRef, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, LogLevel,
    LogRecord, NodeName, RunAttachEventNotification, RunAttachParams, RunAttachResult,
    RunForceParams, RunForceResult, RunId, RunLogEventNotification, RunLogsParams, RunLogsResult,
    RunMetadata, RunSize, RunStatus, RunStatusParams, RunStatusResult, RunTitle,
    RunWatchEventNotification, RunWatchParams, RunWatchResult, SourceBranchId, SourceRepositoryId,
    SourceRevisionId, ResolvedSource, SubscriptionId, TerminalResult, TokenCount, TokenUsage,
    UnixTimestampMillis,
};
use schemars::{schema_for, JsonSchema};
use serde_json::{json, Value};

use crate::artifacts::{json_artifact, Artifact};

const ROOT: &str = "protocol/openengine-cluster/v1";

#[derive(JsonSchema)]
pub struct NativeV2ObservationSchema {
    pub status_request: JsonRpcRequest<RunStatusParams>,
    pub status_response: JsonRpcResponse<RunStatusResult>,
    pub watch_request: JsonRpcRequest<RunWatchParams>,
    pub watch_response: JsonRpcResponse<RunWatchResult>,
    pub watch_event_notification: JsonRpcNotification<RunWatchEventNotification>,
    pub logs_request: JsonRpcRequest<RunLogsParams>,
    pub logs_response: JsonRpcResponse<RunLogsResult>,
    pub log_event_notification: JsonRpcNotification<RunLogEventNotification>,
    pub attach_request: JsonRpcRequest<RunAttachParams>,
    pub attach_response: JsonRpcResponse<RunAttachResult>,
    pub attach_event_notification: JsonRpcNotification<RunAttachEventNotification>,
    pub suspend_request: JsonRpcRequest<openengine_cluster_protocol::RunSuspendParams>,
    pub suspend_response: JsonRpcResponse<openengine_cluster_protocol::RunSuspendResult>,
    pub inbox_list_request: JsonRpcRequest<openengine_cluster_protocol::InboxListParams>,
    pub inbox_list_response: JsonRpcResponse<openengine_cluster_protocol::InboxListResult>,
    pub inbox_show_request: JsonRpcRequest<openengine_cluster_protocol::InboxShowParams>,
    pub inbox_show_response: JsonRpcResponse<openengine_cluster_protocol::InboxShowResult>,
    pub vectors_request: JsonRpcRequest<openengine_cluster_protocol::RunVectorsParams>,
    pub vectors_response: JsonRpcResponse<openengine_cluster_protocol::RunVectorsResult>,
    pub vector_request: JsonRpcRequest<openengine_cluster_protocol::RunVectorParams>,
    pub vector_response: JsonRpcResponse<openengine_cluster_protocol::RunVectorResult>,
    pub force_request: JsonRpcRequest<RunForceParams>,
    pub force_response: JsonRpcResponse<RunForceResult>,
}

pub(crate) fn artifacts() -> Vec<Artifact> {
    let schema = serde_json::to_value(schema_for!(NativeV2ObservationSchema))
        .assert_value_with("native-v2 observation schema serialization must succeed");
    let mut result = crate::request_artifacts::artifacts();
    result.extend(vec![
        json_artifact(
            format!("{ROOT}/fixtures/native_v2_observation/suspended-command.json"),
            suspension_fixture(false),
        ),
        json_artifact(
            format!("{ROOT}/fixtures/native_v2_observation/suspended-wait.json"),
            suspension_fixture(true),
        ),
        json_artifact(
            format!("{ROOT}/fixtures/native_v2_observation/suspend-requested.json"),
            {
                let mut status = status();
                if let RunStatus::Running {
                    suspend_requested, ..
                } = &mut status.status
                {
                    *suspend_requested = true;
                }
                serde_json::to_value(status).assert_value()
            },
        ),
        json_artifact(format!("{ROOT}/native-v2-observation.schema.json"), schema),
        json_artifact(
            format!("{ROOT}/fixtures/native_v2_observation/status.json"),
            serde_json::to_value(status()).assert_value(),
        ),
        json_artifact(
            format!("{ROOT}/fixtures/native_v2_observation/status-unavailable.json"),
            {
                let mut result = status();
                result.controller = Some(openengine_cluster_protocol::ControllerObservation {
                    state: openengine_cluster_protocol::ControllerState::Lost,
                    observed_at: UnixTimestampMillis::new(1_794_470_707_000).assert_value(),
                });
                serde_json::to_value(result).assert_value()
            },
        ),
        json_artifact(
            format!("{ROOT}/fixtures/native_v2_observation/watch.json"),
            watch_fixture(),
        ),
        json_artifact(
            format!("{ROOT}/fixtures/native_v2_observation/graph_result.json"),
            graph_result_fixture(),
        ),
        json_artifact(
            format!("{ROOT}/fixtures/native_v2_observation/logs.json"),
            logs_fixture(),
        ),
        json_artifact(
            format!("{ROOT}/fixtures/native_v2_observation/attach.json"),
            attach_fixture(),
        ),
        json_artifact(
            format!("{ROOT}/fixtures/native_v2_observation/force.json"),
            force_fixture(),
        ),
    ]);
    result
}

fn run_id() -> RunId {
    RunId::new("run-1")
}

fn cursor(value: u64) -> Cursor {
    Cursor::new(format!("v2:{value}"))
}

fn execution(value: &str) -> ExecutionRef {
    ExecutionRef::new(value).assert_value()
}

fn title() -> RunTitle {
    RunTitle::new("Repair checkout flow").assert_value()
}

pub fn native_v2_source_fixture() -> ResolvedSource {
    ResolvedSource {
        repository: SourceRepositoryId::new("open-engine/zeroshot").assert_value(),
        branch: SourceBranchId::new("main").assert_value(),
        revision: SourceRevisionId::new("0123456789abcdef0123456789abcdef01234567").assert_value(),
        remote: None,
    }
}

fn status() -> RunStatusResult {
    RunStatusResult {
        run_id: run_id(),
        title: title(),
        source: Some(native_v2_source_fixture()),
        size: RunSize::new("medium").assert_value(),
        at_cursor: cursor(7),
        status: RunStatus::Running {
            suspend_requested: false,
            graph_result: None,
            active_executions: vec![
                ActiveExecution {
                    map_indices: Vec::new(),
                    execution: execution("opaque-verifier-a"),
                    node: NodeName::new("verify-a").assert_value(),
                },
                ActiveExecution {
                    map_indices: Vec::new(),
                    execution: execution("opaque-verifier-b"),
                    node: NodeName::new("verify-b").assert_value(),
                },
            ],
        },
        controller: None,
        workspace_recovery: Default::default(),
    }
}

fn watch_fixture() -> Value {
    json!({
        "params": RunWatchParams {
            run_id: run_id(),
            from_cursor: Some(cursor(7)),
        },
        "result": RunWatchResult {
            subscription_id: SubscriptionId::new("watch-1"),
            run_id: run_id(),
            at_cursor: cursor(7),
        },
        "event": RunWatchEventNotification {
            subscription_id: SubscriptionId::new("watch-1"),
            run_id: run_id(),
            title: title(),
            source: Some(native_v2_source_fixture()),
            size: RunSize::new("medium").assert_value(),
            cursor: cursor(8),
            status: RunStatus::Finished {
                terminal_result: TerminalResult::Succeeded {
                    output: json!({
                        "kind": "verification_receipt",
                        "summary": "checkout repaired",
                        "passed": true
                    }),
                },
                metadata: RunMetadata {
                    token_usage: Some(TokenUsage {
                        input_tokens: TokenCount::new(1_200).assert_value(),
                        output_tokens: TokenCount::new(200).assert_value(),
                        cache_read_input_tokens: Some(TokenCount::new(800).assert_value()),
                        cache_creation_input_tokens: Some(TokenCount::new(100).assert_value()),
                        complete: true,
                    }),
                delivery_receipt: None,
                },
            },
        }
    })
}

fn graph_result_fixture() -> Value {
    let mut result = status();
    result.at_cursor = cursor(8);
    result.status = RunStatus::Running {
        suspend_requested: false,
        active_executions: Vec::new(),
        graph_result: Some(TerminalResult::Succeeded {
            output: json!({"summary":"graph completed; postmortem is running"}),
        }),
    };
    serde_json::to_value(result).assert_value()
}

fn logs_fixture() -> Value {
    json!({
        "params": RunLogsParams {
            run_id: run_id(),
            from_cursor: Some(cursor(7)),
            execution: Some(execution("opaque-verifier-b")),
        },
        "result": RunLogsResult {
            subscription_id: SubscriptionId::new("logs-1"),
            run_id: run_id(),
            at_cursor: cursor(7),
        },
        "event": RunLogEventNotification {
            map_indices: Vec::new(), subscription_id: SubscriptionId::new("logs-1"),
            run_id: run_id(),
            cursor: cursor(9),
            timestamp: UnixTimestampMillis::new(1_725_000_000_123).assert_value(),
            execution: Some(execution("opaque-verifier-b")),
            record: LogRecord {
                level: LogLevel::Info,
                target: BoundedLogTarget::new("agent").assert_value(),
                message: BoundedLogMessage::new("historical output").assert_value(),
            },
        }
    })
}

fn attach_fixture() -> Value {
    json!({
        "params": RunAttachParams {
            run_id: run_id(),
            execution: execution("opaque-verifier-b"),
        },
        "result": RunAttachResult {
            subscription_id: SubscriptionId::new("attach-1"),
            run_id: run_id(),
            execution: execution("opaque-verifier-b"),
        },
        "event": RunAttachEventNotification {
            subscription_id: SubscriptionId::new("attach-1"),
            run_id: run_id(),
            execution: execution("opaque-verifier-b"),
            event: AgentAttachEvent::Output {
                text: BoundedAssistantOutput::new("live output").assert_value(),
            },
        }
    })
}

fn force_fixture() -> Value {
    let mut result = status();
    result.at_cursor = cursor(10);
    let active_executions = match result.status {
        RunStatus::Running {
            active_executions, ..
        } => Some(active_executions),
        _ => None,
    }
    .assert_value_with("fixture status must be running");
    result.status = RunStatus::Stopping {
        active_executions,
        graph_result: None,
    };
    json!({
        "params": RunForceParams { run_id: run_id() },
        "result": RunForceResult {
            run_id: result.run_id,
            title: result.title,
            source: result.source,
            size: result.size,
            at_cursor: result.at_cursor,
            status: result.status,
            controller: None,
            workspace_recovery: result.workspace_recovery,
        }
    })
}

fn suspension_fixture(wait: bool) -> Value {
    use openengine_cluster_protocol::{RunSuspension, RunWait};
    let mut status = status();
    let node = NodeName::new("cooldown").assert_value();
    let created_at = UnixTimestampMillis::new(1_762_905_600_000).assert_value();
    let suspension = if wait {
        RunSuspension::Wait {
            node,
            created_at,
            wait: RunWait {
                due_at: UnixTimestampMillis::new(1_762_992_000_000).assert_value(),
            },
        }
    } else {
        RunSuspension::Command { node, created_at }
    };
    status.status = RunStatus::Suspended {
        suspension,
        metadata: RunMetadata::default(),
    };
    serde_json::to_value(status).assert_value()
}
