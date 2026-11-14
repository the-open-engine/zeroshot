use super::*;
use openengine_cluster_protocol::{
    InboxListParams, RequestState, RunResumeFrom, RunVectorSummaryContent,
};
fn ask_request() -> RunSubmitParams {
    let mut request = wait_request();
    let mut graph = serde_json::to_value(&request.submission.graph).assert_value();
    let schema =
        json!({"kind":"record","fields":{"text":{"required":true,"type":{"kind":"string"}}}});
    graph["initialInput"] = schema.clone();
    graph["root"]["state"] = schema.clone();
    let node = &mut graph["root"]["children"][0];
    node["worker"] = json!(crate::native_v2_request::ASK_WORKER_REF);
    node["input"] = schema;
    node["output"] = json!({"kind":"record","fields":{}});
    node["signals"] = json!({"decision":["approved","rejected"]});
    node["timeoutMs"] = json!(1000);
    node["inputBindings"] = json!([{"target":["text"],"value":{"source":"state","path":["text"]}}]);
    let worker = graph["root"]["children"][1].clone();
    graph["root"]["children"][1] = json!({"kind":"choice","name":"timeout_route","state":graph["root"]["state"],"branches":[{"when":{"kind":"in","value":{"name":"wait","source":"error","field":null},"labels":["timeout"]},"node":{"kind":"succeed","name":"expired","output":{"kind":"null"},"bindings":[]}}],"otherwise":worker,"promotedStatePaths":[]});
    request.submission.graph = serde_json::from_value(graph).assert_value();
    request.submission.initial_input = json!({"text":"Approve?"});
    request
}
async fn suspended(controller: &NativeV2PlatformController, run: &RunId) -> RunStatusResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = controller
                .status(RunStatusParams {
                    run_id: run.clone(),
                })
                .await
                .assert_value();
            if matches!(status.status, RunStatus::Suspended { .. }) {
                controller.wait_runtime_settled(run).await;
                return status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value()
}
#[tokio::test]
async fn request_answer_at_deadline_is_timeout_and_before_it_is_accepted_once() {
    for expired in [false, true] {
        let driver = Arc::new(FakeDriver::new(Behavior::Complete));
        let (ledger, _, runtime) = runtime_parts(driver.clone());
        let clock = Arc::new(AtomicU64::new(1000));
        let allocator = Arc::new(SuspensionAllocator {
            runtime,
            policy: crate::native_v2_wait::WaitPolicy {
                wall_clock: {
                    let clock = clock.clone();
                    Arc::new(move || {
                        UnixTimestampMillis::new(clock.load(Ordering::SeqCst)).assert_value()
                    })
                },
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
        let run = submit_test_request(&controller, ask_request())
            .await
            .assert_value()
            .run_id;
        let status = suspended(&controller, &run).await;
        assert!(!status.workspace_recovery.recoverable);
        assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
        let mut params = resume_params(&run);
        params.from = Some(RunResumeFrom::Answer {
            signals: serde_json::from_value(json!({"decision":"approved"})).assert_value(),
            output: json!({}),
        });
        let mut invalid = params.clone();
        invalid.from = None;
        invalid.connections.clear();
        assert!(
            controller
                .resume(invalid)
                .await
                .unwrap_err()
                .to_string()
                .contains("zeroshot answer")
        );
        let mut invalid = params.clone();
        invalid.from = Some(RunResumeFrom::Answer {
            signals: Default::default(),
            output: json!({}),
        });
        assert!(
            controller
                .resume(invalid)
                .await
                .unwrap_err()
                .to_string()
                .contains("signals.decision")
        );
        assert_eq!(ledger.list().await.assert_value().len(), 1);
        clock.store(if expired { 2000 } else { 1999 }, Ordering::SeqCst);
        let list = controller
            .inbox_list(InboxListParams::default())
            .await
            .assert_value();
        assert_eq!(list.requests.len(), 1);
        assert_eq!(
            list.requests[0].state,
            if expired {
                RequestState::Expired
            } else {
                RequestState::Pending
            }
        );
        let result = controller.resume(params.clone()).await;
        if expired {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("expired"), "{error}");
        } else {
            result.assert_value();
        }
        assert!(matches!(
            terminal(&controller, &params.successor_run_id).await,
            TerminalResult::Succeeded { .. }
        ));
        assert_eq!(
            driver.starts.load(Ordering::SeqCst),
            if expired { 0 } else { 1 }
        );
        let old = controller
            .inbox_show(RunStatusParams {
                run_id: run.clone(),
            })
            .await
            .assert_value();
        assert_eq!(old.successor_run_id, Some(params.successor_run_id.clone()));
        assert_eq!(
            old.state,
            if expired {
                RequestState::Expired
            } else {
                RequestState::Answered
            }
        );
        assert!(
            controller
                .inbox_list(InboxListParams::default())
                .await
                .assert_value()
                .requests
                .is_empty()
        );
        assert!(
            controller
                .inbox_show(RunStatusParams {
                    run_id: params.successor_run_id.clone()
                })
                .await
                .is_err()
        );
        let vectors = controller
            .vectors(RunVectorsParams {
                run_id: params.successor_run_id.clone(),
                after: None,
                limit: None,
            })
            .await
            .assert_value();
        assert!(vectors.vectors.iter().any(|entry| matches!(
            (&entry.content, expired),
            (RunVectorSummaryContent::Expiry { .. }, true)
                | (RunVectorSummaryContent::Answer { .. }, false)
        )));
        assert!(
            controller
                .resume(params)
                .await
                .unwrap_err()
                .to_string()
                .contains("wait-successor")
        );
        assert_eq!(ledger.list().await.assert_value().len(), 2);
    }
}
#[tokio::test]
async fn host_requires_the_independent_request_opt_in() {
    let harness = harness(Behavior::Hang).await;
    assert!(matches!(
        submit_test_request(&harness.controller, ask_request()).await,
        Err(NativeV2PlatformError::Admission(_))
    ));
    assert!(harness.ledger.list().await.assert_value().is_empty());
}
