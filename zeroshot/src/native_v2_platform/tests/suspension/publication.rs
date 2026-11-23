use super::*;
use openengine_cluster_protocol::{RunSteerParams, SteeringMode};

fn steer(run_id: &RunId, text: &str) -> RunSteerParams {
    RunSteerParams {
        run_id: run_id.clone(),
        node: None,
        text: text.to_owned(),
        mode: SteeringMode::Standing,
    }
}

async fn refused(controller: &NativeV2PlatformController, run: &RunId) {
    let before = controller
        .ledger
        .get(run)
        .await
        .assert_value()
        .assert_value()
        .snapshot;
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        controller.steer(steer(run, "must not be acknowledged")),
    )
    .await
    .assert_value_with("steering must not wait for publication or disposal")
    .assert_error();
    assert!(
        matches!(&error, NativeV2PlatformError::InvalidRequest(message)
        if message.contains("suspension") && message.contains("again shortly")),
        "{error}"
    );
    assert_eq!(
        controller
            .ledger
            .get(run)
            .await
            .assert_value()
            .assert_value()
            .snapshot,
        before
    );
}

async fn hosted() -> (
    NativeV2PlatformController,
    Arc<SuspensionAllocator>,
    Arc<PublicationStore>,
    RunId,
) {
    let driver = Arc::new(FakeDriver::new(Behavior::Complete));
    let (ledger, _, runtime) = runtime_parts(driver);
    let store = Arc::new(PublicationStore::default());
    let allocator = Arc::new(SuspensionAllocator {
        runtime,
        policy: Default::default(),
        store: Arc::new(SuspensionStore {
            publication: Some(store.clone()),
            ..Default::default()
        }),
        successor: StdMutex::new(None),
        preparation_gate: None,
    });
    let controller = NativeV2PlatformController::new_with_delivery_policy(
        ledger,
        allocator.clone(),
        DeliveryPolicy::Optional,
    )
    .await
    .assert_value();
    let run = submit_test_request(&controller, wait_request())
        .await
        .assert_value()
        .run_id;
    while !matches!(controller.status(RunStatusParams { run_id: run.clone() }).await.assert_value().status,
        RunStatus::Running { ref active_executions, .. } if active_executions.iter().any(|e| e.node.as_str() == "wait"))
    {
        tokio::task::yield_now().await;
    }
    (controller, allocator, store, run)
}

#[tokio::test]
async fn host_publication_refuses_steers_during_save_disposal_and_after_engine_removal() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (controller, allocator, store, run) = hosted().await;
        let ledger = controller.ledger.clone();
        // In-place waits leave admission open until the suspension handoff starts.
        let first = controller.steer(steer(&run, "  first\t方向\r\n")).await.assert_value();
        let second = controller.steer(steer(&run, "second")).await.assert_value();
        controller.suspend(RunSuspendParams { run_id: run.clone() }).await.assert_value();
        store.published.notified().await;
        refused(&controller, &run).await;
        let published = store.vectors.lock().assert_value().clone();
        let accepted: Vec<_> = published.iter().filter(|v| matches!(v.content, RunVectorContent::Steer { .. })).collect();
        assert_eq!(accepted.iter().map(|v| &v.id).collect::<Vec<_>>(), [&first.steer.id, &second.steer.id]);
        assert!(matches!(&accepted[0].content, RunVectorContent::Steer { text, .. } if text == "  first\t方向\r\n"));
        assert!(matches!(&accepted[1].content, RunVectorContent::Steer { text, .. } if text == "second"));

        store.publish_release.notify_one();
        store.disposing.notified().await;
        refused(&controller, &run).await;
        store.disposal_release.notify_one();
        controller.wait_runtime_settled(&run).await;
        while controller.runtimes.lock().await.contains_key(&run) {
            tokio::task::yield_now().await;
        }
        refused(&controller, &run).await;
        let durable = crate::v2_run_ledger::vectors::list_full(ledger.as_ref(), RunVectorsParams {
            run_id: run.clone(), after: None, limit: None,
        }).await.assert_value().vectors;
        assert_eq!(durable, published);
        assert_eq!(*store.vectors.lock().assert_value(), published);
        drop(controller);
        let controller = NativeV2PlatformController::new_with_delivery_policy(
            ledger, allocator, DeliveryPolicy::Optional,
        ).await.assert_value();
        refused(&controller, &run).await;
    }).await.assert_value();
}

#[tokio::test]
async fn failed_host_publication_does_not_acknowledge_a_late_steer() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (controller, _, store, run) = hosted().await;
        let accepted = controller.steer(steer(&run, "before handoff")).await.assert_value();
        controller.suspend(RunSuspendParams { run_id: run.clone() }).await.assert_value();
        store.published.notified().await;
        refused(&controller, &run).await;
        store.reject.store(true, Ordering::SeqCst);
        store.publish_release.notify_one();
        controller.wait_runtime_settled(&run).await;
        assert!(matches!(controller.status(RunStatusParams { run_id: run.clone() }).await.assert_value().status,
            RunStatus::Finished { terminal_result: TerminalResult::Failed { reason, .. }, .. } if reason.as_str() == "runtime_failed"));
        assert!(!store.exits.lock().assert_value().contains(&RunRuntimeExit::Suspended));
        let directions = crate::v2_run_ledger::steering::all(controller.ledger.as_ref(), &run).await.assert_value();
        assert_eq!(directions.len(), 1);
        assert_eq!(directions[0].id, accepted.steer.id);
    }).await.assert_value();
}

struct ReceivingDriver {
    hooks: Arc<crate::active_memory::hooks::ActiveMemoryHooks>,
    prompts: StdMutex<Vec<String>>,
}

#[async_trait]
impl NodeDriver for ReceivingDriver {
    fn delivery_hooks(&self) -> Option<Arc<crate::active_memory::hooks::ActiveMemoryHooks>> {
        Some(self.hooks.clone())
    }

    async fn run(
        &self,
        mut invocation: DriverInvocation,
        _: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        self.hooks
            .prepare_steers(&mut invocation, false)
            .await
            .assert_value();
        self.prompts
            .lock()
            .assert_value()
            .push(invocation.host_context.clone().unwrap_or_default());
        self.hooks
            .finish_steers(invocation.node.reference.execution.get(), true)
            .await;
        Ok(fake_worker_outcome(&invocation))
    }
}

#[tokio::test]
async fn retained_ledger_accepts_suspended_steers_and_delivers_them_in_order_to_successor() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let driver = Arc::new(ReceivingDriver {
            hooks: Arc::new(crate::active_memory::hooks::ActiveMemoryHooks::empty().assert_value()),
            prompts: StdMutex::default(),
        });
        let (ledger, _, runtime) = runtime_parts(driver.clone());
        let clock = Arc::new(AtomicU64::new(1_762_905_600_000));
        let read_clock = clock.clone();
        let allocator = Arc::new(SuspensionAllocator {
            runtime,
            policy: crate::native_v2_wait::WaitPolicy {
                in_place_threshold: Some(Duration::from_secs(3600)),
                wall_clock: Arc::new(move || {
                    UnixTimestampMillis::new(read_clock.load(Ordering::SeqCst)).assert_value()
                }),
            },
            store: Arc::new(SuspensionStore {
                retained_ledger: true,
                ..Default::default()
            }),
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
        let run = submit_test_request(&controller, wait_request())
            .await
            .assert_value()
            .run_id;
        while !matches!(
            controller
                .status(RunStatusParams {
                    run_id: run.clone()
                })
                .await
                .assert_value()
                .status,
            RunStatus::Suspended { .. }
        ) {
            tokio::task::yield_now().await;
        }
        controller.wait_runtime_settled(&run).await;
        let first_text = "  retained-first\t方向\r\n";
        let first = controller
            .steer(steer(&run, first_text))
            .await
            .assert_value();
        drop(controller);
        let controller = NativeV2PlatformController::new_with_delivery_policy(
            ledger.clone(),
            allocator,
            DeliveryPolicy::Optional,
        )
        .await
        .assert_value();
        let second = controller
            .steer(steer(&run, "retained-second"))
            .await
            .assert_value();
        let before = crate::v2_run_ledger::steering::all(ledger.as_ref(), &run)
            .await
            .assert_value();
        assert_eq!(
            before.iter().map(|v| &v.id).collect::<Vec<_>>(),
            [&first.steer.id, &second.steer.id]
        );
        clock.fetch_add(7_200_000, Ordering::SeqCst);
        let next = controller
            .resume(resume_params(&run))
            .await
            .assert_value()
            .run_id;
        assert!(matches!(
            terminal(&controller, &next).await,
            TerminalResult::Succeeded { .. }
        ));
        let inherited = crate::v2_run_ledger::steering::all(ledger.as_ref(), &next)
            .await
            .assert_value();
        assert_eq!(inherited[..before.len()], before);
        let receipts: Vec<_> = inherited[before.len()..]
            .iter()
            .map(|vector| {
                let RunVectorContent::SteerReceipt { steer_id, .. } = &vector.content else {
                    panic!("successor must acknowledge the inherited steers");
                };
                steer_id
            })
            .collect();
        assert_eq!(receipts, [&first.steer.id, &second.steer.id]);
        let prompts = driver.prompts.lock().assert_value();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains(first_text));
        assert!(prompts[0].find(first_text) < prompts[0].find("retained-second"));
    })
    .await
    .assert_value();
}
