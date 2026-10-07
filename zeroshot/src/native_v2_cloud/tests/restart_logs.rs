use super::*;
use crate::full_v1_reducer::{ExecutionId, NodeInstanceId, StructuralOccurrence};
use crate::native_v2_contract::{ExecutionRef, NodeCompletion};
use openengine_cluster_protocol::{LogLevel, PositiveInteger, RunLogEventNotification};

async fn read_logs(
    controller: &NativeV2CloudController,
    params: RunLogsParams,
) -> Vec<RunLogEventNotification> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let (_, mut source) = controller.logs(params).await.assert_value();
        let mut records = Vec::new();
        while let Some(record) = source.recv().await.assert_value() {
            records.push(record);
        }
        records
    })
    .await
    .assert_value_with("terminal run logs drain")
}

fn assert_same_records(actual: &[RunLogEventNotification], expected: &[RunLogEventNotification]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.cursor, expected.cursor);
        assert_eq!(actual.timestamp, expected.timestamp);
        assert_eq!(actual.execution, expected.execution);
        assert_eq!(actual.record, expected.record);
    }
}

fn node_start(run_id: &RunId, name: &str, identity: u64) -> RunEvent {
    let node = NodeName::new(name).assert_value();
    RunEvent::NodeStarted {
        reference: ExecutionRef {
            run_id: run_id.clone(),
            node: node.clone(),
            node_instance: NodeInstanceId::new(identity).assert_value(),
            execution: ExecutionId::new(identity).assert_value(),
        },
        occurrence: StructuralOccurrence {
            node,
            map_indices: Vec::new(),
        },
        attempt: PositiveInteger::new(1).assert_value(),
        input: Value::Null,
    }
}

async fn interrupted_parallel_run(ledger: &Arc<FakeRunLedger>) -> RunId {
    let request = complex_request();
    let admitted = NativeV2Admission
        .admit(request.submission.clone())
        .await
        .assert_value();
    ledger
        .create_or_get(CreateRun {
            run_id: request.run_id.clone(),
            submission_key: request.submission.submission_key.clone(),
            submission_digest: submission_digest(&request.submission).assert_value(),
            admitted,
        })
        .await
        .assert_value();
    let worker = node_start(&request.run_id, "worker", 1);
    let RunEvent::NodeStarted { reference, .. } = &worker else {
        unreachable!("node_start creates a node start");
    };
    let completion = RunEvent::NodeCompleted {
        completion: NodeCompletion {
            reference: reference.clone(),
            outcome: WorkerOutcome::Verified {
                output: Value::Null,
                artifacts: Vec::new(),
            },
        },
    };
    ledger
        .append(
            &request.run_id,
            vec![
                RunEvent::RunStarted,
                worker,
                completion,
                node_start(&request.run_id, "left", 2),
                node_start(&request.run_id, "right", 3),
            ],
        )
        .await
        .assert_value();
    request.run_id
}

#[tokio::test]
async fn restarted_active_nodes_keep_public_failure_logs_and_replay_without_duplication() {
    let Harness {
        controller,
        ledger,
        allocator,
        ..
    } = harness(Behavior::Complete).await;
    let run_id = interrupted_parallel_run(&ledger).await;
    drop(controller);
    let controller = NativeV2CloudController::new(ledger.clone(), allocator.clone())
        .await
        .assert_value();
    let params = || RunLogsParams {
        run_id: run_id.clone(),
        from_cursor: None,
        execution: None,
    };
    let records = read_logs(&controller, params()).await;
    assert_eq!(records.len(), 3);
    for (record, name) in records.iter().take(2).zip(["left", "right"]) {
        assert_eq!(record.record.level, LogLevel::Error);
        assert!(record.execution.is_some());
        assert_eq!(
            record.record.message.as_str(),
            format!("Node {name} failed: crash: run runtime_lost")
        );
    }
    assert_ne!(records[0].execution, records[1].execution);
    assert!(records[2].execution.is_none());
    assert_eq!(records[2].record.level, LogLevel::Error);
    assert_eq!(
        records[2].record.message.as_str(),
        "Run failed: runtime_lost"
    );

    let filtered = read_logs(
        &controller,
        RunLogsParams {
            execution: records[0].execution.clone(),
            ..params()
        },
    )
    .await;
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].cursor, records[0].cursor);
    assert_eq!(filtered[0].record, records[0].record);
    let replay = read_logs(
        &controller,
        RunLogsParams {
            from_cursor: Some(records[0].cursor.clone()),
            ..params()
        },
    )
    .await;
    assert_same_records(&replay, &records[1..]);
    let filtered_replay = read_logs(
        &controller,
        RunLogsParams {
            from_cursor: Some(records[0].cursor.clone()),
            execution: records[0].execution.clone(),
            ..params()
        },
    )
    .await;
    assert!(filtered_replay.is_empty());

    let before = controller
        .status(RunStatusParams {
            run_id: run_id.clone(),
        })
        .await
        .assert_value();
    drop(controller);
    let reconstructed = NativeV2CloudController::new(ledger, allocator.clone())
        .await
        .assert_value();
    let after = reconstructed
        .status(RunStatusParams { run_id })
        .await
        .assert_value();
    assert_eq!(after, before);
    let persisted = read_logs(
        &reconstructed,
        RunLogsParams {
            run_id: after.run_id,
            from_cursor: None,
            execution: None,
        },
    )
    .await;
    assert_same_records(&persisted, &records);
    assert_eq!(allocator.allocation_count(), 0);
}
