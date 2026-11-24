use std::sync::Arc;
use openengine_cluster_protocol::{
    HostedRunStatusResult, HostedRunWatchEventNotification, IdempotencyKey, NodeName,
    PositiveInteger, RunId, RunStatusParams, RunWatchParams, Sha256Digest, TerminalResult,
    UnixTimestampMillis,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use zeroshot_engine::full_v1_reducer::StructuralOccurrence;
use zeroshot_engine::native_v2_contract::{ExecutionId, ExecutionRef, NodeInstanceId};
use zeroshot_engine::v2_run_ledger::{CreateRun, RunEvent, RunLedger, fake::FakeRunLedger};

fn rejects_timing<T: DeserializeOwned>(mut value: Value, field: &str) {
    // The released reader also predates logical IDs. Isolate timing in this control.
    fn remove_parts(row: &mut Value) {
        if let Some(fields) = row.as_object_mut() {
            fields.remove("partId");
            fields.remove("currentPartId");
        }
    }
    if let Some(rows) = value.get_mut("runs").and_then(Value::as_array_mut) {
        rows.iter_mut().for_each(remove_parts);
    } else {
        remove_parts(&mut value);
    }
    let error = serde_json::from_value::<T>(value.clone())
        .err()
        .expect("old reader rejects timing");
    assert!(error.to_string().contains(field), "{error}");
    let mut control = value;
    strip_timing(&mut control);
    assert!(
        serde_json::from_value::<T>(control).is_ok(),
        "removing only timing restores decoding"
    );
}

fn strip_timing(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.remove("startedAt");
            fields.remove("recordedAt");
            fields.values_mut().for_each(strip_timing);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_timing),
        _ => {}
    }
}

#[tokio::test]
async fn exact_released_readers_reject_actual_producer_timing_without_the_contract_gate() {
    let submission = serde_json::from_value(json!({
        "title":"Wire control", "initialInput":null, "submissionKey":"wire",
        "graph":{
            "profile":"openengine.graph.full/v1", "initialInput":{"kind":"null"},
            "policy":{"policy":"policy.native-v2@1","default":"deny"},
            "root":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        },
        "runtime":{"harness":"codex","provider":"openai","size":"medium","nodes":{}}
    }))
    .expect("submission");
    let admitted = zeroshot_engine::native_v2_admission::NativeV2Admission
        .admit(submission)
        .await
        .expect("admit");
    let ledger = Arc::new(FakeRunLedger::new().with_wall_clock(Arc::new(|| {
        UnixTimestampMillis::new(10_000).expect("clock")
    })));
    let run_id = RunId::new("wire");
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            admitted,
            submission_key: IdempotencyKey::new("wire").expect("key"),
            submission_digest: Sha256Digest::new("a".repeat(64)).expect("digest"),
        })
        .await
        .expect("create");
    let node = NodeName::new("done").expect("node");
    let reference = ExecutionRef {
        run_id: run_id.clone(),
        node: node.clone(),
        execution: ExecutionId::new(37).expect("execution"),
        node_instance: NodeInstanceId::new(11).expect("instance"),
    };
    ledger
        .append(
            &run_id,
            vec![
                RunEvent::RunStarted,
                RunEvent::NodeStarted {
                    reference: reference.clone(),
                    occurrence: StructuralOccurrence {
                        node,
                        map_indices: vec![2],
                    },
                    attempt: PositiveInteger::new(1).expect("attempt"),
                    input: Value::Null,
                },
            ],
        )
        .await
        .expect("start");
    let service =
        zeroshot_engine::native_v2_observability::NativeV2Observability::new(ledger.clone());
    let active = service
        .status(RunStatusParams::new(run_id.clone()))
        .await
        .expect("active status");
    rejects_timing::<released_protocol::RunStatusResult>(
        serde_json::to_value(&active).expect("serialize"),
        "startedAt",
    );
    rejects_timing::<released_protocol::HostedRunStatusResult>(
        serde_json::to_value(HostedRunStatusResult::from(active.clone())).expect("serialize"),
        "untagged enum",
    );
    let (_, mut watch) = service
        .watch(RunWatchParams {
            part_id: None,
            run_id: run_id.clone(),
            from_cursor: None,
        })
        .await
        .expect("watch");
    let starts = watch.read_available().await.expect("active notifications");
    let active_event = starts.last().expect("node start");
    assert!(serde_json::to_value(active_event).expect("serialize")["status"]["activeExecutions"][0]["startedAt"].is_number());
    rejects_timing::<released_protocol::RunWatchEventNotification>(
        serde_json::to_value(active_event).expect("serialize"),
        "recordedAt",
    );
    rejects_timing::<released_protocol::HostedRunWatchEventNotification>(
        serde_json::to_value(HostedRunWatchEventNotification::from(active_event.clone()))
            .expect("serialize"),
        "recordedAt",
    );
    ledger
        .append(
            &run_id,
            vec![
                RunEvent::NodeCompleted {
                    completion: zeroshot_engine::native_v2_contract::NodeCompletion {
                        reference,
                        outcome: openengine_cluster_protocol::WorkerOutcome::Verified {
                            output: Value::Null,
                            artifacts: Vec::new(),
                        },
                    },
                },
                RunEvent::Terminal {
                    result: TerminalResult::Succeeded {
                        output: Value::Null,
                    },
                },
            ],
        )
        .await
        .expect("finish");
    let finished = service
        .status(RunStatusParams::new(run_id))
        .await
        .expect("finished status");
    rejects_timing::<released_protocol::RunListResult>(
        json!({"runs":[&active,&finished]}),
        "startedAt",
    );
    rejects_timing::<released_protocol::HostedRunListResult>(
        json!({"runs":[HostedRunStatusResult::from(active),HostedRunStatusResult::from(finished)]}),
        "untagged enum",
    );
    let events = watch.read_available().await.expect("finished notification");
    let event = events.last().expect("finished event");
    rejects_timing::<released_protocol::RunWatchEventNotification>(
        serde_json::to_value(event).expect("serialize"),
        "recordedAt",
    );
    rejects_timing::<released_protocol::HostedRunWatchEventNotification>(
        serde_json::to_value(HostedRunWatchEventNotification::from(event.clone()))
            .expect("serialize"),
        "recordedAt",
    );
}
