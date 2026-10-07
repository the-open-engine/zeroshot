use super::*;

#[tokio::test]
async fn force_cancels_preparation_without_waiting_for_allocation_or_starting_nodes() {
    let GatedHarness {
        controller,
        ledger,
        cleanup,
        allocator,
    } = gated_harness().await;
    let submission = request(Value::Null);
    let receipt = tokio::time::timeout(
        Duration::from_millis(250),
        submit_test_request(&controller, submission.clone()),
    )
    .await
    .assert_value()
    .assert_value();
    allocator.wait_started().await;
    let replay = tokio::time::timeout(
        Duration::from_millis(250),
        submit_test_request(&controller, submission),
    )
    .await
    .assert_value()
    .assert_value();
    assert!(replay.deduped);
    assert_eq!(replay.run_id, receipt.run_id);
    assert!(cleanup.exits().is_empty());
    let forced = tokio::time::timeout(
        Duration::from_secs(2),
        controller.force(RunForceParams {
            run_id: receipt.run_id.clone(),
        }),
    )
    .await
    .assert_value()
    .assert_value();
    assert!(
        matches!(forced.status, RunStatus::Finished { terminal_result: TerminalResult::Failed { reason }, .. }
        if reason.as_str() == "force_stopped")
    );
    assert_eq!(allocator.allocation_count(), 1);
    assert_eq!(cleanup.exits(), vec![RunRuntimeExit::ForceStopped]);
    assert_eq!(cleanup.terminal_seen(), vec![false]);
    let stored = ledger
        .get(&receipt.run_id)
        .await
        .assert_value()
        .assert_value();
    assert!(stored.snapshot.executions.is_empty());
    allocator.release();
}

#[tokio::test]
async fn capsule_loss_confirms_absence_then_terminalizes_without_replacement() {
    let (harness, receipt) = started_harness(Behavior::Hang).await;
    harness.allocator.lose_capsule();
    assert_failed_cleanup(
        &harness,
        &receipt.run_id,
        "runtime_lost",
        RunRuntimeExit::RuntimeLost,
    )
    .await;
    let replay = submit_test_request(&harness.controller, request(Value::Null))
        .await
        .assert_value_with("resubmit");
    assert!(replay.deduped);
    assert_eq!(replay.run_id, receipt.run_id);
    assert_eq!(harness.allocator.allocation_count(), 1);
}

#[tokio::test]
async fn exact_resubmit_of_controller_reconstructed_run_confirms_absence_without_allocation() {
    let harness = harness(Behavior::Complete).await;
    let run_id = seed_controller_reconstructed_run(&harness.ledger, "run-orphaned").await;
    let replay = submit_test_request(&harness.controller, request(Value::Null))
        .await
        .assert_value_with("resubmit");
    assert!(replay.deduped);
    assert_eq!(replay.run_id, run_id);
    assert_eq!(harness.allocator.allocation_count(), 0);
    assert_failed_cleanup(
        &harness,
        &run_id,
        "runtime_lost",
        RunRuntimeExit::RuntimeLost,
    )
    .await;
}

use openengine_cluster_testkit::assertions::{AssertValue};
