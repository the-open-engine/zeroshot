use crate::fixture::*;

use openengine_cluster_protocol::{
    AgentAttachParams, ApplyParams, DeleteParams, ResubmitParams, RetryParams, RunAttachParams,
    RunCheckpointsParams, RunDiscardWorkspaceParams, RunForceParams, RunLogsParams,
    RunResumeParams, RunStatusParams, RunSubmitParams, RunWatchParams, StopParams, UpdateParams,
    RUN_ATTACH_METHOD, RUN_CHECKPOINTS_METHOD, RUN_DISCARD_WORKSPACE_METHOD, RUN_FORCE_METHOD,
    RUN_LIST_METHOD, RUN_LOGS_METHOD, RUN_RESUME_METHOD, RUN_STATUS_METHOD, RUN_SUBMIT_METHOD,
    RUN_WATCH_METHOD,
};
use openengine_cluster_server::method_registry::{MethodDescriptor, MethodKind, METHOD_REGISTRY};
use schemars::schema_for;
use serde_json::{json, Value};

pub(super) fn document() -> Value {
    let methods = METHOD_REGISTRY
        .iter()
        .map(method_document)
        .collect::<Vec<_>>();
    json!({
        "openrpc": "1.3.2",
        "info": {
            "title": "Open Engine Cluster Protocol",
            "version": "1.0.0"
        },
        "methods": methods,
        "components": {
            "schemas": {
                "GraphSpec": { "$ref": "graph.schema.json" },
                "CompiledGraphIr": { "$ref": "compiled-ir.schema.json" },
                "GraphDiagnostic": { "$ref": "graph.schema.json#/$defs/GraphDiagnostic" },
                "StructuralBounds": { "$ref": "graph.schema.json#/$defs/StructuralBounds" },
                "ArtifactRef": { "$ref": "graph.schema.json#/$defs/ArtifactRef" }
            }
        },
        "x-generic-subscription-framing": {
                "description": "`watch`, `logs`, and `agent/attach` each establish a subscription \
                through one JSON-RPC result. `run/watch`, `run/logs`, and `run/attach` use the same \
                framing. The connection layer establishes all six subscriptions; \
                `Dispatcher::dispatch` alone \
                cannot answer them. After establishment, every subscription uses the generic \
                notification methods below. No method-specific event, cancel, or closed names \
                exist on the wire: `watch/event`, `watch/cancel`, `watch/closed`, `logs/event`, \
                `logs/cancel`, `logs/closed`, `agent/attach/event`, `agent/attach/cancel`, and \
                `agent/attach/closed` are not methods. `$/cancelRequest` asks the transport to cancel \
                an in-flight unary request by its RequestId. An unknown or already-completed ID is \
                ignored, and cancellation makes no rollback claim after backend state commits.",
            "notifications": {
                "event": { "$ref": "schema.json#/$defs/EventNotification" },
                "subscription/cancel": { "$ref": "schema.json#/$defs/SubscriptionCancelParams" },
                "subscription/closed": { "$ref": "schema.json#/$defs/SubscriptionClosedNotification" },
                "$/cancelRequest": { "$ref": "schema.json#/$defs/CancelRequestParams" }
            }
        }
    })
}
fn method_document(descriptor: &MethodDescriptor) -> Value {
    let mut method = standard_method(descriptor.name)
        .or_else(|| lifecycle_method(descriptor.name))
        .or_else(|| observation_method(descriptor.name))
        .or_else(|| native_v2_method(descriptor.name))
        .assert_value_with("METHOD_REGISTRY method must have an OpenRPC schema");
    let object = method
        .as_object_mut()
        .assert_value_with("OpenRPC method builders must return objects");
    object.insert("name".to_owned(), json!(descriptor.name));
    object.insert(
        "x-subscription".to_owned(),
        json!(matches!(descriptor.kind, MethodKind::Subscription(_))),
    );
    object.insert(
        "x-transport-requirements".to_owned(),
        json!({
            "serverPush": descriptor.transport_requirements.server_push,
            "inboundNotifications": descriptor.transport_requirements.inbound_notifications,
        }),
    );
    method
}

fn standard_method(name: &str) -> Option<Value> {
    match name {
        "initialize" => initialize_method(),
        "plan" => plan_method(),
        "apply" => apply_method(),
        "get" => get_method(),
        _ => return None,
    }
    .into()
}

fn lifecycle_method(name: &str) -> Option<Value> {
    match name {
        "update" => update_method(),
        "stop" => stop_method(),
        "retry" => retry_method(),
        "resubmit" => resubmit_method(),
        "delete" => delete_method(),
        _ => return None,
    }
    .into()
}

fn observation_method(name: &str) -> Option<Value> {
    match name {
        "watch" => watch_method(),
        "logs" => logs_method(),
        "agent/attach" => agent_attach_method(),
        _ => return None,
    }
    .into()
}

fn native_v2_method(name: &str) -> Option<Value> {
    let method = match name {
        RUN_SUBMIT_METHOD => run_submit_method(),
        RUN_LIST_METHOD => run_list_method(),
        RUN_STATUS_METHOD => run_status_method(),
        RUN_WATCH_METHOD => run_watch_method(),
        RUN_LOGS_METHOD => run_logs_method(),
        RUN_ATTACH_METHOD => run_attach_method(),
        RUN_FORCE_METHOD => run_force_method(),
        _ => return native_v2_recovery_method(name),
    };
    Some(method)
}

fn native_v2_recovery_method(name: &str) -> Option<Value> {
    match name {
        RUN_CHECKPOINTS_METHOD => run_checkpoints_method(),
        RUN_RESUME_METHOD => run_resume_method(),
        RUN_DISCARD_WORKSPACE_METHOD => run_discard_workspace_method(),
        _ => return None,
    }
    .into()
}

fn initialize_method() -> Value {
    json!({
        "paramStructure": "by-name",
        "params": [{
            "name": "protocolVersion",
            "required": true,
            "schema": {
                "type": "string",
                "const": "openengine.cluster/v1"
            }
        }],
        "result": {
            "name": "initializeResult",
            "schema": { "$ref": "schema.json#/$defs/InitializeResult" }
        }
    })
}

fn plan_method() -> Value {
    json!({
        "paramStructure": "by-name",
        "params": [{
            "name": "graph",
            "required": true,
            "schema": { "$ref": "schema.json#/$defs/GraphSpec" }
        }],
        "result": {
            "name": "planResult",
            "schema": { "$ref": "schema.json#/$defs/PlanResult" }
        }
    })
}

fn apply_method() -> Value {
    let apply_schema = serde_json::to_value(schema_for!(ApplyParams))
        .assert_value_with("apply parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [
            {
                "name": "graph", "required": true,
                "schema": { "$ref": "schema.json#/$defs/GraphSpec" }
            },
            { "name": "input", "required": false, "schema": true },
            {
                "name": "dryRun", "required": false,
                "schema": apply_property_schema(&apply_schema, "dryRun")
            },
            {
                "name": "ifGeneration", "required": false,
                "schema": apply_property_schema(&apply_schema, "ifGeneration")
            },
            {
                "name": "idempotencyKey", "required": false,
                "schema": apply_property_schema(&apply_schema, "idempotencyKey")
            }
        ],
        "result": {
            "name": "applyResult",
            "schema": { "$ref": "schema.json#/$defs/ApplyResult" }
        }
    })
}

fn apply_property_schema(apply_schema: &Value, property: &str) -> Value {
    apply_schema
        .assert_key("properties")
        .get(property)
        .assert_value_with("ApplyParams schema must contain every documented property")
        .clone()
}

fn update_method() -> Value {
    let schema = serde_json::to_value(schema_for!(UpdateParams))
        .assert_value_with("update parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "x-params-schema": schema,
        "params": [
            { "name": "labels", "required": false, "schema": { "$ref": "schema.json#/$defs/Labels" } },
            { "name": "logLevel", "required": false, "schema": { "$ref": "schema.json#/$defs/LogLevel" } },
            { "name": "suspended", "required": false, "schema": { "type": "boolean" } },
            { "name": "ifGeneration", "required": true, "schema": property_schema(&schema, "ifGeneration") },
            { "name": "idempotencyKey", "required": true, "schema": property_schema(&schema, "idempotencyKey") }
        ],
        "result": {
            "name": "updateResult",
            "schema": { "$ref": "schema.json#/$defs/UpdateResult" }
        }
    })
}

fn stop_method() -> Value {
    let schema = serde_json::to_value(schema_for!(StopParams))
        .assert_value_with("stop parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [
            { "name": "mode", "required": true, "schema": { "$ref": "schema.json#/$defs/StopMode" } },
            { "name": "ifGeneration", "required": true, "schema": property_schema(&schema, "ifGeneration") },
            { "name": "idempotencyKey", "required": true, "schema": property_schema(&schema, "idempotencyKey") }
        ],
        "result": {
            "name": "stopResult",
            "schema": { "$ref": "schema.json#/$defs/StopResult" }
        }
    })
}

fn retry_method() -> Value {
    let schema = serde_json::to_value(schema_for!(RetryParams))
        .assert_value_with("retry parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [
            { "name": "ifGeneration", "required": true, "schema": property_schema(&schema, "ifGeneration") },
            { "name": "idempotencyKey", "required": true, "schema": property_schema(&schema, "idempotencyKey") }
        ],
        "result": {
            "name": "retryResult",
            "schema": { "$ref": "schema.json#/$defs/RetryResult" }
        }
    })
}

fn resubmit_method() -> Value {
    let schema = serde_json::to_value(schema_for!(ResubmitParams))
        .assert_value_with("resubmit parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [
            { "name": "ifGeneration", "required": true, "schema": property_schema(&schema, "ifGeneration") },
            { "name": "ifRunId", "required": true, "schema": property_schema(&schema, "ifRunId") },
            { "name": "idempotencyKey", "required": true, "schema": property_schema(&schema, "idempotencyKey") },
            { "name": "replacementInput", "required": false, "schema": true }
        ],
        "result": {
            "name": "resubmitResult",
            "schema": { "$ref": "schema.json#/$defs/ResubmitResult" }
        }
    })
}

fn delete_method() -> Value {
    let schema = serde_json::to_value(schema_for!(DeleteParams))
        .assert_value_with("delete parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [
            { "name": "ifGeneration", "required": true, "schema": property_schema(&schema, "ifGeneration") },
            { "name": "ifRunId", "required": false, "schema": property_schema(&schema, "ifRunId") },
            { "name": "idempotencyKey", "required": true, "schema": property_schema(&schema, "idempotencyKey") }
        ],
        "result": {
            "name": "deleteResult",
            "schema": { "$ref": "schema.json#/$defs/DeleteResult" }
        }
    })
}

fn property_schema(schema: &Value, property: &str) -> Value {
    schema
        .assert_key("properties")
        .get(property)
        .assert_value_with("parameter schema must contain every documented property")
        .clone()
}

fn get_method() -> Value {
    json!({
        "paramStructure": "by-name",
        "params": [{
            "name": "atCursor",
            "required": false,
            "schema": { "type": ["string", "null"] }
        }],
        "result": {
            "name": "getResult",
            "schema": { "$ref": "schema.json#/$defs/GetResult" }
        }
    })
}

fn watch_method() -> Value {
    json!({
        "paramStructure": "by-name",
        "params": [
            {
                "name": "runId", "required": false,
                "schema": { "type": ["string", "null"] }
            },
            {
                "name": "fromCursor", "required": false,
                "schema": { "type": ["string", "null"] }
            }
        ],
        "result": {
            "name": "watchResult",
            "schema": { "$ref": "schema.json#/$defs/WatchResult" }
        }
    })
}

fn logs_method() -> Value {
    json!({
        "paramStructure": "by-name",
        "params": [],
        "result": {
            "name": "logsResult",
            "schema": { "$ref": "schema.json#/$defs/LogsResult" }
        }
    })
}

fn agent_attach_method() -> Value {
    // `ExecutionRef` is inline-schema (like `logs`'s `BoundedLogTarget`/`BoundedLogMessage`), so it
    // has no standalone `$defs` entry to `$ref` -- extract its actual inline schema from a
    // generated `AgentAttachParams` schema instead of hand-authoring a `$ref` that would dangle.
    let schema = serde_json::to_value(schema_for!(AgentAttachParams))
        .assert_value_with("agent_attach parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [{
            "name": "execution",
            "required": true,
            "schema": property_schema(&schema, "execution")
        }],
        "result": {
            "name": "agentAttachResult",
            "schema": { "$ref": "schema.json#/$defs/AgentAttachResult" }
        }
    })
}

fn run_submit_method() -> Value {
    let schema = serde_json::to_value(schema_for!(RunSubmitParams))
        .assert_value_with("run submit parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [
            { "name": "runId", "required": true, "schema": property_schema(&schema, "runId") },
            { "name": "submission", "required": true, "schema": { "$ref": "schema.json#/$defs/RunSubmission" } }
        ],
        "result": {
            "name": "runSubmitResult",
            "schema": { "$ref": "schema.json#/$defs/RunSubmitResult" }
        }
    })
}

fn run_list_method() -> Value {
    json!({
        "paramStructure": "by-name",
        "params": [],
        "result": {
            "name": "runListResult",
            "schema": { "$ref": "schema.json#/$defs/RunListResult" }
        }
    })
}

fn run_status_method() -> Value {
    run_id_method::<RunStatusParams>("runStatusResult", "RunStatusResult")
}

fn run_force_method() -> Value {
    run_id_method::<RunForceParams>("runForceResult", "RunForceResult")
}

fn run_discard_workspace_method() -> Value {
    run_id_method::<RunDiscardWorkspaceParams>(
        "runDiscardWorkspaceResult",
        "RunDiscardWorkspaceResult",
    )
}

fn run_checkpoints_method() -> Value {
    let schema = serde_json::to_value(schema_for!(RunCheckpointsParams))
        .assert_value_with("checkpoint parameter JSON Schema serialization must succeed");
    json!({
        "summary": "List retained node and atomic-group entry checkpoints",
        "description": "Lists checkpoints in increasing sequence order. The after cursor is exclusive. \
            Omitted limit means 50; accepted limits are 1 through 100. Concurrent groups, including \
            every map wave and nested child, expose one entry point. Storage and execution seeds \
            remain private to the target.",
        "paramStructure": "by-name",
        "params": [
            {"name": "runId", "required": true, "schema": property_schema(&schema, "runId")},
            {"name": "after", "required": false, "schema": property_schema(&schema, "after")},
            {"name": "limit", "required": false, "schema": property_schema(&schema, "limit")}
        ],
        "result": {
            "name": "runCheckpointsResult",
            "schema": { "$ref": "schema.json#/$defs/RunCheckpointsResult" }
        }
    })
}

fn run_resume_method() -> Value {
    let schema = serde_json::to_value(schema_for!(RunResumeParams))
        .assert_value_with("run resume parameter JSON Schema serialization must succeed");
    json!({
        "summary": "Admit a successor from a retained workspace or entry checkpoint",
        "description": "Omitting from or selecting restart starts the graph at its root on the latest \
            retained workspace. Selecting checkpoint restores the matching workspace and predecessor \
            outputs and reruns the named node or atomic concurrent group. Original admission and \
            delivery metadata remain fixed. Credentials are resolved again for the successor; provider \
            sessions are not resumed.",
        "paramStructure": "by-name",
        "params": [
            {"name": "runId", "required": true, "schema": property_schema(&schema, "runId")},
            {"name": "successorRunId", "required": true, "schema": property_schema(&schema, "successorRunId")},
            {"name": "from", "required": false, "schema": property_schema(&schema, "from")},
            {"name": "connections", "required": false, "schema": property_schema(&schema, "connections")},
            {"name": "connectionResolver", "required": false, "schema": property_schema(&schema, "connectionResolver")},
            {"name": "githubToken", "required": false, "schema": property_schema(&schema, "githubToken")}
        ],
        "result": {
            "name": "runResumeResult",
            "schema": { "$ref": "schema.json#/$defs/RunResumeResult" }
        }
    })
}

fn run_id_method<P: schemars::JsonSchema>(result_name: &str, result_type: &str) -> Value {
    let schema = serde_json::to_value(schema_for!(P))
        .assert_value_with("native-v2 run parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [{
            "name": "runId", "required": true,
            "schema": property_schema(&schema, "runId")
        }],
        "result": {
            "name": result_name,
            "schema": { "$ref": format!("schema.json#/$defs/{result_type}") }
        }
    })
}

fn run_watch_method() -> Value {
    let schema = serde_json::to_value(schema_for!(RunWatchParams))
        .assert_value_with("run watch parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [
            { "name": "runId", "required": true, "schema": property_schema(&schema, "runId") },
            { "name": "fromCursor", "required": false, "schema": property_schema(&schema, "fromCursor") }
        ],
        "result": {
            "name": "runWatchResult",
            "schema": { "$ref": "schema.json#/$defs/RunWatchResult" }
        }
    })
}

fn run_logs_method() -> Value {
    let schema = serde_json::to_value(schema_for!(RunLogsParams))
        .assert_value_with("run logs parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [
            { "name": "runId", "required": true, "schema": property_schema(&schema, "runId") },
            { "name": "fromCursor", "required": false, "schema": property_schema(&schema, "fromCursor") },
            { "name": "execution", "required": false, "schema": property_schema(&schema, "execution") }
        ],
        "result": {
            "name": "runLogsResult",
            "schema": { "$ref": "schema.json#/$defs/RunLogsResult" }
        }
    })
}

fn run_attach_method() -> Value {
    let schema = serde_json::to_value(schema_for!(RunAttachParams))
        .assert_value_with("run attach parameter JSON Schema serialization must succeed");
    json!({
        "paramStructure": "by-name",
        "params": [
            { "name": "runId", "required": true, "schema": property_schema(&schema, "runId") },
            { "name": "execution", "required": true, "schema": property_schema(&schema, "execution") }
        ],
        "result": {
            "name": "runAttachResult",
            "schema": { "$ref": "schema.json#/$defs/RunAttachResult" }
        }
    })
}
