use super::*;

#[tokio::test]
async fn force_of_controller_reconstructed_run_confirms_absence_and_finishes() {
    let harness = harness(Behavior::Complete).await;
    let run_id = seed_controller_reconstructed_run(&harness.ledger, "run-force-orphaned").await;
    let forced = harness
        .controller
        .force(RunForceParams {
            run_id: run_id.clone(),
        })
        .await
        .assert_value_with("force");
    assert!(matches!(forced.status, RunStatus::Finished { .. }));
    assert_eq!(harness.allocator.allocation_count(), 0);
    assert_eq!(harness.cleanup.exits(), vec![RunRuntimeExit::ForceStopped]);
    assert_eq!(harness.cleanup.terminal_seen(), vec![false]);
}

#[tokio::test]
async fn concurrent_force_of_reconstructed_run_cleans_up_and_terminalizes_once() {
    let harness = harness(Behavior::Complete).await;
    let run_id = seed_controller_reconstructed_run(&harness.ledger, "run-concurrent-force").await;
    let left_controller = harness.controller.clone();
    let left_id = run_id.clone();
    let left = tokio::spawn(async move {
        left_controller
            .force(RunForceParams { run_id: left_id })
            .await
    });
    let right_controller = harness.controller.clone();
    let right_id = run_id.clone();
    let right = tokio::spawn(async move {
        right_controller
            .force(RunForceParams { run_id: right_id })
            .await
    });
    let (left, right) = tokio::join!(left, right);
    assert!(matches!(
        left.assert_value_with("left task")
            .assert_value_with("left force")
            .status,
        RunStatus::Finished { .. }
    ));
    assert!(matches!(
        right
            .assert_value_with("right task")
            .assert_value_with("right force")
            .status,
        RunStatus::Finished { .. }
    ));
    assert_eq!(harness.cleanup.exits(), vec![RunRuntimeExit::ForceStopped]);
    let tail = harness
        .ledger
        .snapshot_and_tail(&run_id, None)
        .await
        .assert_value_with("tail");
    assert_eq!(
        tail.events
            .iter()
            .filter(|event| matches!(event.event, RunEvent::Terminal { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn drive_cleanup_failure_automatically_retries_and_reports_runtime_failure() {
    let harness = harness(Behavior::Complete).await;
    harness.cleanup.fail_next();
    let receipt = submit_test_request(&harness.controller, request(Value::Null))
        .await
        .assert_value_with("submit");
    assert_eq!(
        terminal(&harness.controller, &receipt.run_id).await,
        TerminalResult::Failed {
            reason: EnumLabel::new("runtime_failed").assert_value_with("label")
        }
    );
    assert_eq!(
        harness.cleanup.exits(),
        vec![RunRuntimeExit::Completed, RunRuntimeExit::RuntimeLost]
    );
    assert_eq!(harness.cleanup.terminal_seen(), vec![false, false]);

    let forced = harness
        .controller
        .force(RunForceParams {
            run_id: receipt.run_id.clone(),
        })
        .await
        .assert_value_with("force after automatic recovery");
    assert!(matches!(forced.status, RunStatus::Finished { .. }));
    assert_eq!(harness.cleanup.exits().len(), 2);
    assert!(
        harness
            .ledger
            .get(&receipt.run_id)
            .await
            .assert_value_with("ledger")
            .assert_value_with("stored")
            .snapshot
            .terminal
            .is_some()
    );
}

#[tokio::test]
async fn allocation_cleanup_refusal_remains_nonterminal_until_recovery_confirms_absence() {
    let harness = harness(Behavior::Complete).await;
    harness
        .allocator
        .fail_next_allocation(CapsuleAllocationUnavailable::SourceCheckout);
    harness.cleanup.fail_next();
    let submission = request(Value::Null);
    assert!(matches!(
        submit_test_request(&harness.controller, submission.clone()).await,
        Err(NativeV2CloudError::Supervisor(
            NativeV2SupervisorError::RuntimeCleanup(_)
        ))
    ));
    let stored = harness
        .ledger
        .get_by_submission_key(&submission.submission.submission_key)
        .await
        .assert_value_with("ledger read")
        .assert_value_with("failed allocation remains durable");
    assert!(stored.snapshot.terminal.is_none());
    assert_eq!(harness.cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
    assert_eq!(harness.cleanup.terminal_seen(), vec![false]);
    assert_eq!(harness.allocator.allocation_count(), 1);
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 0);

    let receipt = submit_test_request(&harness.controller, submission)
        .await
        .assert_value_with("retry confirms cleanup");
    assert!(receipt.deduped);
    assert_eq!(receipt.run_id, stored.snapshot.run_id);
    assert_eq!(
        terminal(&harness.controller, &receipt.run_id).await,
        TerminalResult::Failed {
            reason: EnumLabel::new("runtime_lost").assert_value_with("label")
        }
    );
    assert_eq!(
        harness.cleanup.exits(),
        vec![RunRuntimeExit::RuntimeLost, RunRuntimeExit::RuntimeLost]
    );
    assert_eq!(harness.cleanup.terminal_seen(), vec![false, false]);
    assert_eq!(harness.allocator.allocation_count(), 1);
    assert_eq!(harness.driver.starts.load(Ordering::SeqCst), 0);
}

use openengine_cluster_testkit::assertions::{AssertValue};
