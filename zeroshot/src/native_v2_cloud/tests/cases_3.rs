use super::*;

async fn assert_active_submission_conflicts(
    controller: &NativeV2CloudController,
    ledger: &FakeRunLedger,
    allocator: &FakeAllocator,
    first: &CloudRunReceipt,
) {
    let exact = submit_test_request(controller, request_with_key(Value::Null, "cloud-first"))
        .await
        .assert_value();
    assert!(exact.deduped);
    assert_eq!(exact.run_id, first.run_id);

    let mut conflicting_reuse = request_with_key(Value::Null, "cloud-first");
    let mut graph = serde_json::to_value(&conflicting_reuse.submission.graph).assert_value();
    *graph
        .pointer_mut("/root/children/0/timeoutMs")
        .assert_value() = json!(9_999);
    conflicting_reuse.submission.graph = serde_json::from_value(graph).assert_value();
    let conflict = submit_test_request(controller, conflicting_reuse)
        .await
        .assert_error();
    assert!(matches!(
        conflict,
        NativeV2CloudError::Ledger(RunLedgerError::SubmissionConflict { existing_run_id })
            if existing_run_id == first.run_id
    ));

    wait_for_allocations(allocator, 1).await;
    assert_eq!(ledger.list().await.assert_value().len(), 1);
}

async fn gated_controller(
    ledger: Arc<FakeRunLedger>,
    allocator: Arc<GatedAllocator>,
) -> NativeV2CloudController {
    NativeV2CloudController::new(ledger, allocator)
        .await
        .assert_value_with("controller startup")
}

#[tokio::test]
async fn distinct_nonterminal_runs_are_both_admitted() {
    let ledger = Arc::new(FakeRunLedger::new());
    let driver = Arc::new(FakeDriver::new(Behavior::Hang));
    let cleanup = Arc::new(FakeCleanup::new(ledger.clone()));
    let allocator = Arc::new(GatedAllocator::new(driver, cleanup));
    let controller = gated_controller(ledger.clone(), allocator.clone()).await;
    let first_request = request_with_key(Value::Null, "cloud-first");
    let second_request = request_with_key(Value::Null, "cloud-second");

    let first_controller = controller.clone();
    let first =
        tokio::spawn(
            async move { submit_test_request(&first_controller, first_request.clone()).await },
        );
    allocator.wait_started().await;
    let second_controller = controller.clone();
    let second =
        tokio::spawn(async move { submit_test_request(&second_controller, second_request).await });

    wait_for_allocations(allocator.as_ref(), 2).await;

    allocator.release();
    let first = first
        .await
        .assert_value_with("first task")
        .assert_value_with("first submission");
    let second = second
        .await
        .assert_value_with("second task")
        .assert_value_with("second submission");
    assert_ne!(second.run_id, first.run_id);
    assert_eq!(allocator.allocation_count(), 2);
    assert_eq!(
        ledger
            .list()
            .await
            .assert_value_with("list after concurrent submissions")
            .len(),
        2
    );

    controller
        .force(RunForceParams {
            run_id: first.run_id.clone(),
        })
        .await
        .assert_value_with("first force stop");
    terminal(&controller, &first.run_id).await;

    controller
        .force(RunForceParams {
            run_id: second.run_id.clone(),
        })
        .await
        .assert_value_with("second force stop");
    terminal(&controller, &second.run_id).await;
}

#[tokio::test]
async fn exact_retry_dedupes_and_changed_retry_conflicts() {
    let harness = harness(Behavior::Hang).await;
    let first = submit_test_request(
        &harness.controller,
        request_with_key(Value::Null, "cloud-first"),
    )
    .await
    .assert_value_with("first submission");
    assert_active_submission_conflicts(
        &harness.controller,
        harness.ledger.as_ref(),
        harness.allocator.as_ref(),
        &first,
    )
    .await;
    harness
        .controller
        .force(RunForceParams {
            run_id: first.run_id.clone(),
        })
        .await
        .assert_value_with("cleanup");
}

#[tokio::test]
async fn exact_source_revision_participates_in_retry_identity() {
    let harness = harness(Behavior::Hang).await;
    let request = request_with_key(Value::Null, "cloud-branch");
    let digest = submission_digest(&request.submission).assert_value_with("submission digest");
    let environment = exact_test_environment(&request).assert_value_with("branch test environment");
    let first = harness
        .controller
        .submit_with_exact_environment(request.clone(), environment)
        .await
        .assert_value_with("first branch submission");

    let exact = harness
        .controller
        .resolve_submission(&request.submission.submission_key, &digest)
        .await
        .assert_value_with("exact retry lookup")
        .assert_value_with("exact retry receipt");
    assert_eq!(exact.run_id, first.run_id);

    let mut changed_submission = request.submission.clone();
    changed_submission.source.revision =
        SourceRevisionId::new("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .assert_value_with("changed revision");
    let changed = submission_digest(&changed_submission).assert_value_with("changed digest");
    assert!(matches!(
        harness
            .controller
            .resolve_submission(&request.submission.submission_key, &changed)
            .await,
        Err(NativeV2CloudError::Ledger(
            RunLedgerError::SubmissionConflict { .. }
        ))
    ));
    wait_for_allocations(harness.allocator.as_ref(), 1).await;
    harness
        .controller
        .force(RunForceParams {
            run_id: first.run_id,
        })
        .await
        .assert_value_with("cleanup");
}

#[tokio::test]
async fn detached_submitter_does_not_cancel_preparation_or_allow_duplicate_allocation() {
    let GatedHarness {
        controller,
        ledger,
        cleanup,
        allocator,
    } = gated_harness().await;
    let owner = controller.clone();
    let (accepted, receipt) = tokio::sync::oneshot::channel();
    let submitter = tokio::spawn(async move {
        let result = submit_test_request(&owner, request_with_key(Value::Null, "detached")).await;
        let _ = accepted.send(result);
        std::future::pending::<()>().await;
    });
    let receipt = receipt.await.assert_value().assert_value();
    allocator.wait_started().await;
    submitter.abort();
    assert!(submitter.await.assert_error().is_cancelled());
    let replay = submit_test_request(&controller, request_with_key(Value::Null, "detached"))
        .await
        .assert_value();
    assert!(replay.deduped);
    assert_eq!(replay.run_id, receipt.run_id);
    assert!(cleanup.exits().is_empty());
    assert_eq!(allocator.allocation_count(), 1);
    assert!(
        ledger
            .get(&receipt.run_id)
            .await
            .assert_value()
            .assert_value()
            .snapshot
            .terminal
            .is_none()
    );
    allocator.release();
    controller
        .force(RunForceParams {
            run_id: receipt.run_id,
        })
        .await
        .assert_value();
}

#[tokio::test]
async fn wave9_cli_contract_settled_allocation_refusal_cleans_up_before_terminal_failure() {
    let harness = harness(Behavior::Complete).await;
    harness
        .allocator
        .fail_next_allocation(CapsuleAllocationUnavailable::Runtime);
    let request = request_with_key(Value::Null, "cloud-wave9-allocation-refusal");
    let run_id = request.run_id.clone();
    submit_test_request(&harness.controller, request)
        .await
        .assert_value_with("accepted before allocation");
    assert_eq!(
        terminal(&harness.controller, &run_id).await,
        TerminalResult::Failed {
            reason: EnumLabel::new("runtime_unavailable").assert_value_with("failure label")
        }
    );
    assert_eq!(harness.cleanup.exits(), vec![RunRuntimeExit::RuntimeLost]);
    assert_eq!(harness.cleanup.terminal_seen(), vec![false]);
}

#[tokio::test]
async fn force_destroys_live_capsule_before_one_terminal_result() {
    let (harness, receipt) = started_harness(Behavior::Hang).await;
    harness
        .controller
        .force(RunForceParams {
            run_id: receipt.run_id.clone(),
        })
        .await
        .assert_value_with("force");
    assert_failed_cleanup(
        &harness,
        &receipt.run_id,
        "force_stopped",
        RunRuntimeExit::ForceStopped,
    )
    .await;
    let tail = harness
        .ledger
        .snapshot_and_tail(&receipt.run_id, None)
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

use openengine_cluster_testkit::assertions::{AssertValue, AssertError};
