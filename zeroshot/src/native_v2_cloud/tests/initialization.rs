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
        .allocate(&run_id, &stored.admitted, None)
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
    start.capsule.execution_seed = vec![DurableExecution {
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
    }];
    start
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
        Err(NativeV2CloudError::Ledger(RunLedgerError::EventTooLarge))
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
    assert!(matches!(result, Err(NativeV2CloudError::Observation(_))));
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
        Err(NativeV2CloudError::Supervisor(
            NativeV2SupervisorError::RuntimeCleanup(_)
        ))
    ));
    assert_eq!(harness.cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
    assert_eq!(harness.cleanup.terminal_seen(), vec![false]);
    assert_not_launched(&harness, &run_id, false).await;

    let replacement =
        NativeV2CloudController::new(harness.ledger.clone(), harness.allocator.clone())
            .await
            .assert_value();
    assert_eq!(
        terminal(&replacement, &run_id).await,
        TerminalResult::Failed {
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
