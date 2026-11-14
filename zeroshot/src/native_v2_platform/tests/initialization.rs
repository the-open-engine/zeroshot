use super::*;
use crate::full_v1_reducer::{
    DurableExecution, DurableExecutionState, ExecutionId, HistoryPosition, NodeInstanceId,
    StructuralOccurrence,
};
use crate::v2_run_ledger::{MAX_PRIOR_EXECUTION_BYTES, RunPhase};
use openengine_cluster_protocol::PositiveInteger;

async fn allocated_start(harness: &Harness) -> AllocatedRunStart {
    let run_id =
        seed_controller_reconstructed_run(&harness.ledger, "allocated-initialization").await;
    let stored = harness
        .ledger
        .get(&run_id)
        .await
        .assert_value()
        .assert_value();
    let capsule = harness
        .allocator
        .allocate(CapsuleAllocationRequest {
            run_id: &run_id,
            admitted: &stored.admitted,
            github_token: None,
            git_credentials: None,
            commit_identity: None,
            commit_co_author: None,
            preparation: test_preparation(),
        })
        .await
        .assert_value();
    let controller_claim = harness
        .allocator
        .claim_controller(&run_id)
        .await
        .assert_value();
    AllocatedRunStart {
        stored,
        environment: Arc::new(exact_test_environment(&request(Value::Null)).assert_value()),
        controller_claim,
        capsule,
    }
}

async fn oversized_seed_start(harness: &Harness) -> AllocatedRunStart {
    let mut start = allocated_start(harness).await;
    // An explicitly malformed prerequisite exceeds the separate combined-event bound.
    start.capsule.execution_seed = oversized_seed();
    start
}

fn oversized_seed() -> Vec<DurableExecution> {
    vec![DurableExecution {
        delivery_state: None,
        dispatch_position: HistoryPosition::new(1).assert_value(),
        node_instance: NodeInstanceId::new(1).assert_value(),
        execution: ExecutionId::new(1).assert_value(),
        occurrence: StructuralOccurrence {
            node: NodeName::new("worker").assert_value(),
            map_indices: Vec::new(),
        },
        attempt: PositiveInteger::new(1).assert_value(),
        input: Value::String("i".repeat(MAX_PRIOR_EXECUTION_BYTES)),
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(2).assert_value(),
            outcome: WorkerOutcome::Verified {
                output: Value::Null,
                artifacts: Vec::new(),
            },
        },
    }]
}

async fn assert_not_launched(harness: &Harness, run_id: &RunId, terminal_expected: bool) {
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 0);
    assert!(harness.controller.runtimes.lock().await.is_empty());
    assert!(harness.allocator.claim_controller(run_id).await.is_ok());
    let stored = harness
        .ledger
        .get(run_id)
        .await
        .assert_value()
        .assert_value();
    assert!(stored.snapshot.executions.is_empty());
    assert!(stored.snapshot.execution_seed.is_empty());
    assert_eq!(stored.snapshot.terminal.is_some(), terminal_expected);
}

#[tokio::test]
async fn rejected_seed_cleans_allocated_capsule_before_publishing_failure() {
    let harness = harness(Behavior::Complete).await;
    let start = oversized_seed_start(&harness).await;
    let run_id = start.stored.snapshot.run_id.clone();
    let result = harness.controller.start_allocated(start).await;
    assert!(matches!(
        result,
        Err(NativeV2PlatformError::Ledger(RunLedgerError::EventTooLarge))
    ));
    assert_failed_cleanup(
        &harness,
        &run_id,
        "runtime_unavailable",
        RunRuntimeExit::RuntimeLost,
    )
    .await;
    assert_not_launched(&harness, &run_id, true).await;
}

#[tokio::test]
async fn observation_initialization_failure_also_cleans_allocated_capsule() {
    let harness = harness(Behavior::Complete).await;
    let mut start = allocated_start(&harness).await;
    let run_id = start.stored.snapshot.run_id.clone();
    // Simulate a ledger projection that cannot be represented as a runtime observation.
    start.stored.snapshot.phase = RunPhase::Finished;
    let result = harness.controller.start_allocated(start).await;
    assert!(matches!(result, Err(NativeV2PlatformError::Observation(_))));
    assert_failed_cleanup(
        &harness,
        &run_id,
        "runtime_unavailable",
        RunRuntimeExit::RuntimeLost,
    )
    .await;
    assert_not_launched(&harness, &run_id, true).await;
}

#[tokio::test]
async fn rejected_seed_waits_for_confirmed_cleanup_before_terminalization() {
    let harness = harness(Behavior::Complete).await;
    let start = oversized_seed_start(&harness).await;
    let run_id = start.stored.snapshot.run_id.clone();
    harness.cleanup.fail_next();
    let result = harness.controller.start_allocated(start).await;
    assert!(matches!(
        result,
        Err(NativeV2PlatformError::Supervisor(
            NativeV2SupervisorError::RuntimeCleanup(_)
        ))
    ));
    assert_eq!(harness.cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
    assert_eq!(harness.cleanup.terminal_seen(), vec![false]);
    assert_not_launched(&harness, &run_id, false).await;

    let replacement =
        NativeV2PlatformController::new(harness.ledger.clone(), harness.allocator.clone())
            .await
            .assert_value();
    assert_eq!(
        terminal(&replacement, &run_id).await,
        TerminalResult::Failed {
            context: None,
            reason: EnumLabel::new("runtime_lost").assert_value()
        }
    );
    assert_eq!(
        harness.cleanup.exits(),
        vec![RunRuntimeExit::RuntimeLost; 2]
    );
    assert_eq!(harness.cleanup.terminal_seen(), vec![false, false]);
    assert_eq!(harness.allocator.allocation_count(), 1);
}

struct InvalidSeedAllocator(Arc<FakeAllocator>);

#[async_trait]
impl CapsuleAllocator for InvalidSeedAllocator {
    async fn claim_controller(
        &self,
        run_id: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        self.0.claim_controller(run_id).await
    }

    async fn allocate(
        &self,
        request: CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        let mut capsule = self.0.allocate(request).await?;
        capsule.execution_seed = oversized_seed();
        Ok(capsule)
    }

    async fn destroy_or_confirm_absent(
        &self,
        run_id: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        self.0.destroy_or_confirm_absent(run_id, exit).await
    }
}

#[tokio::test]
async fn asynchronous_initialization_failure_releases_preparation_and_controller_authority() {
    let harness = harness(Behavior::Complete).await;
    let controller = NativeV2PlatformController::new(
        harness.ledger.clone(),
        Arc::new(InvalidSeedAllocator(harness.allocator.clone())),
    )
    .await
    .assert_value();
    let receipt = submit_test_request(&controller, request(Value::Null))
        .await
        .assert_value();
    assert!(matches!(terminal(&controller, &receipt.run_id).await,
        TerminalResult::Failed { reason, .. } if reason.as_str() == "runtime_unavailable"));
    tokio::time::timeout(Duration::from_secs(2), async {
        while !controller.runtimes.lock().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value();
    assert!(
        harness
            .allocator
            .claim_controller(&receipt.run_id)
            .await
            .is_ok()
    );
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 0);
    assert_eq!(harness.cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
}

#[tokio::test]
async fn seeded_expiry_logs_before_settlement_and_later_continuations_do_not_repeat_it() {
    use openengine_cluster_protocol::RunVectorContent;
    let harness = harness(Behavior::Complete).await;
    let run = seed_controller_reconstructed_run(&harness.ledger, "expiry-successor").await;
    let mut stored = harness.ledger.get(&run).await.assert_value().assert_value();
    let mut history = oversized_seed();
    history[0].input = Value::Null;
    history[0].state = DurableExecutionState::Settled {
        position: HistoryPosition::new(2).assert_value(),
        outcome: WorkerOutcome::declared_failure(
            openengine_cluster_protocol::WorkerErrorCode::Timeout,
        ),
    };
    let request = crate::v2_run_ledger::vector_event(&RunId::new("request-source"),
        serde_json::from_value(json!({"kind":"request","node":"worker","text":"Approve?",
            "signals":{"decision":["approved"]},"output":{"kind":"record","fields":{}},"expiresAt":2000})).assert_value(),
    ).assert_value();
    let RunEvent::Vector { vector: request } = request else {
        panic!("request vector expected");
    };
    let expiry = crate::v2_run_ledger::vector_event(
        &run,
        RunVectorContent::Expiry {
            request_id: request.id.clone(),
        },
    )
    .assert_value();
    let RunEvent::Vector { vector: expiry } = expiry else {
        panic!("expiry vector expected");
    };
    let vectors = vec![request, expiry];
    harness
        .controller
        .initialize_allocated(&mut stored, history.clone(), vectors.clone())
        .await
        .assert_value();
    let tail = harness
        .ledger
        .snapshot_and_tail(&run, None)
        .await
        .assert_value();
    let errors: Vec<_> = tail
        .events
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            matches!(&record.event,
        RunEvent::SafeLog { line, .. } if line.as_str() == "Node worker failed: timeout")
        })
        .collect();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        tail.events[errors[0].0 + 1].event,
        RunEvent::PriorExecution { .. }
    ));
    assert!(tail.snapshot.failure_context.is_none());

    let runner = NativeNodeRunner::new(
        &stored.admitted,
        harness.driver.clone(),
        Arc::new(FakeSessionFactory::default()),
    )
    .assert_value();
    let supervisor = crate::native_v2_supervisor::NativeV2Supervisor::new(
        run.clone(),
        harness.ledger.clone(),
        Arc::new(runner),
        Arc::new(exact_test_environment(&super::request(Value::Null)).assert_value()),
    );
    supervisor
        .fail_runtime("capsule connection lost", || {})
        .await
        .assert_value();
    let failed = harness.ledger.get(&run).await.assert_value().assert_value();
    let Some(TerminalResult::Failed {
        context: Some(context),
        ..
    }) = failed.snapshot.terminal
    else {
        panic!("runtime failure context expected");
    };
    assert_eq!(
        context.primary.assert_value().message.as_str(),
        "Runtime failed: capsule connection lost"
    );
    assert!(context.recorded.is_empty());

    let mut continuation = super::request(Value::Null);
    continuation.run_id = RunId::new("later-continuation");
    continuation.submission.submission_key =
        IdempotencyKey::new("later-continuation").assert_value();
    persist_request(harness.ledger.as_ref(), &continuation).await;
    let next = continuation.run_id;
    let mut stored = harness
        .ledger
        .get(&next)
        .await
        .assert_value()
        .assert_value();
    harness
        .controller
        .initialize_allocated(&mut stored, history, vectors)
        .await
        .assert_value();
    let tail = harness
        .ledger
        .snapshot_and_tail(&next, None)
        .await
        .assert_value();
    assert!(
        tail.events
            .iter()
            .all(|record| !matches!(record.event, RunEvent::SafeLog { .. }))
    );
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 0);
}
