use super::*;
use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeCompletion, NodeInstanceId};
use crate::native_v2_request::RequestOutcome;
use crate::v2_run_ledger::{MAX_EVENT_BYTES, RunLedgerError, RunSnapshot};
use openengine_cluster_protocol::{RequestResolution, RunResumeFrom, VectorId};

async fn seed_request(ledger: &FakeRunLedger, event: usize) -> RunId {
    let node = NodeName::new("n".repeat(if event == 0 { 128 } else { 1 })).assert_value();
    let mut params = requests::ask_request();
    let mut graph = serde_json::to_value(&params.submission.graph).assert_value();
    let output = json!({"kind":"record","fields":{
        "feedback":{"required":true,"type":{"kind":"string"}}
    }});
    graph["root"]["children"][0]["name"] = json!(node);
    graph["root"]["children"][0]["output"] = output.clone();
    graph["root"]["children"][1]["branches"][0]["when"]["value"]["name"] = json!(node);
    params.submission.graph = serde_json::from_value(graph).assert_value();
    let mut runtime = serde_json::to_value(&params.submission.runtime).assert_value();
    let binding = runtime["nodes"]
        .as_object_mut()
        .assert_value()
        .remove("wait")
        .assert_value();
    runtime["nodes"][node.as_str()] = binding;
    params.submission.runtime = serde_json::from_value(runtime).assert_value();
    let admitted = NativeV2Admission
        .admit_with_policy(params.submission, DeliveryPolicy::Optional)
        .await
        .assert_value();
    let run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c901");
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("answer-bounds").assert_value(),
            submission_digest: openengine_cluster_protocol::Sha256Digest::new("a".repeat(64))
                .assert_value(),
            admitted,
        })
        .await
        .assert_value();
    let reference = ExecutionRef {
        run_id: run_id.clone(),
        node: node.clone(),
        node_instance: NodeInstanceId::new(1).assert_value(),
        execution: ExecutionId::new(1).assert_value(),
    };
    let request = openengine_cluster_protocol::RunRequest {
        id: VectorId::new("q".repeat(64)).assert_value(),
        text: "Approve?".to_owned(),
        signals: serde_json::from_value(json!({"decision":["approved","rejected"]})).assert_value(),
        output: serde_json::from_value(output).assert_value(),
        expires_at: UnixTimestampMillis::new(2000).assert_value(),
    };
    ledger
        .append(
            &run_id,
            vec![
                RunEvent::RunStarted,
                RunEvent::NodeStarted {
                    reference: reference.clone(),
                    occurrence: crate::full_v1_reducer::StructuralOccurrence {
                        node: node.clone(),
                        map_indices: vec![],
                    },
                    attempt: openengine_cluster_protocol::PositiveInteger::new(1).assert_value(),
                    input: json!({"text":"Approve?"}),
                },
                RunEvent::RequestDeferred { reference },
                RunEvent::Vector {
                    vector: RunVector {
                        id: request.id.clone(),
                        run_id: run_id.clone(),
                        created_at: UnixTimestampMillis::MIN,
                        content: RunVectorContent::Request {
                            node: node.clone(),
                            text: request.text.clone(),
                            signals: request.signals.clone(),
                            output: request.output.clone(),
                            expires_at: request.expires_at,
                        },
                    },
                },
                RunEvent::Suspended {
                    suspension: RunSuspension::Request {
                        node,
                        created_at: UnixTimestampMillis::MIN,
                        request: Box::new(request),
                    },
                },
            ],
        )
        .await
        .assert_value();
    run_id
}

fn resolution_events(snapshot: &RunSnapshot, run_id: &RunId, output: &Value) -> [RunEvent; 2] {
    let RunSuspension::Request { request, .. } = snapshot.suspension.as_ref().assert_value() else {
        panic!("request fixture required");
    };
    let resolution = RequestResolution::Answer {
        answer: openengine_cluster_protocol::RequestAnswer {
            signals: serde_json::from_value(json!({"decision":"approved"})).assert_value(),
            output: output.clone(),
        },
    };
    let mut reference = snapshot
        .executions
        .values()
        .next()
        .assert_value()
        .reference
        .clone();
    reference.run_id = run_id.clone();
    [
        RunEvent::NodeCompleted {
            completion: NodeCompletion {
                reference,
                outcome: resolution.outcome(),
            },
        },
        crate::v2_run_ledger::vector_event(
            run_id,
            crate::native_v2_request::resolution_vector(request, &resolution),
        )
        .assert_value(),
    ]
}

#[tokio::test]
async fn successor_event_bounds_are_checked_before_admission() {
    for event in 0..2 {
        let driver = Arc::new(FakeDriver::new(Behavior::Complete));
        let (ledger, _, runtime) = runtime_parts(driver.clone());
        let allocator = Arc::new(SuspensionAllocator {
            runtime,
            policy: crate::native_v2_wait::WaitPolicy {
                wall_clock: Arc::new(|| UnixTimestampMillis::new(1999).assert_value()),
                ..Default::default()
            },
            store: Arc::new(SuspensionStore::default()),
            successor: StdMutex::new(None),
            preparation_gate: None,
        });
        let controller = NativeV2PlatformController::new_with_delivery_policy(
            ledger.clone(),
            allocator.clone(),
            DeliveryPolicy::Optional,
        )
        .await
        .assert_value();
        let run = seed_request(ledger.as_ref(), event).await;
        let before = ledger
            .get(&run)
            .await
            .assert_value()
            .assert_value()
            .snapshot;
        let mut params = resume_params(&run);
        params.successor_run_id = RunId::new("s".repeat(64));
        let overhead = serde_json::to_vec(
            &resolution_events(&before, &params.successor_run_id, &json!({"feedback":""}))[event],
        )
        .assert_value()
        .len();
        let mut output = json!({"feedback":"x".repeat(MAX_EVENT_BYTES - overhead + 1)});
        let events = resolution_events(&before, &params.successor_run_id, &output);
        assert_eq!(
            crate::v2_run_ledger::validate_event(&events[event]),
            Err(RunLedgerError::EventTooLarge)
        );
        crate::v2_run_ledger::validate_event(&events[1 - event]).assert_value();
        // Both events fit when measured with the shorter predecessor identity.
        for event in resolution_events(&before, &run, &output) {
            crate::v2_run_ledger::validate_event(&event).assert_value();
        }
        params.from = Some(RunResumeFrom::Answer {
            signals: serde_json::from_value(json!({"decision":"approved"})).assert_value(),
            output: output.clone(),
        });
        let error = controller.resume(params.clone()).await.unwrap_err();
        assert!(
            matches!(error, NativeV2PlatformError::InvalidRequest(ref message)
            if message == &format!("$.output: {}", RunLedgerError::EventTooLarge)),
            "{error}"
        );
        assert_eq!(ledger.list().await.assert_value().len(), 1);
        assert!(
            ledger
                .get(&params.successor_run_id)
                .await
                .assert_value()
                .is_none()
        );
        assert_eq!(
            ledger
                .get(&run)
                .await
                .assert_value()
                .assert_value()
                .snapshot,
            before
        );
        assert!(allocator.successor.lock().assert_value().is_none());
        assert_eq!(driver.starts.load(Ordering::SeqCst), 0);

        // Removing one byte puts the successor event exactly on the accepted ceiling.
        output["feedback"] = json!("x".repeat(MAX_EVENT_BYTES - overhead));
        assert_eq!(
            serde_json::to_vec(
                &resolution_events(&before, &params.successor_run_id, &output)[event]
            )
            .assert_value()
            .len(),
            MAX_EVENT_BYTES
        );
        if let Some(RunResumeFrom::Answer { output: answer, .. }) = &mut params.from {
            *answer = output;
        }
        controller.resume(params.clone()).await.assert_value();
        assert!(matches!(
            terminal(&controller, &params.successor_run_id).await,
            TerminalResult::Succeeded { .. }
        ));
        assert_eq!(ledger.list().await.assert_value().len(), 2);
        assert_eq!(driver.starts.load(Ordering::SeqCst), 1);
    }
}
