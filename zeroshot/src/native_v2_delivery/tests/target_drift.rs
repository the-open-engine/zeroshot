//! A pr@2 node that declares `opened` in place of `conflict` leaves target drift alone.
use super::*;

const MODE: DeliveryMode = DeliveryMode::PullRequestV2;

fn harness(script: Script) -> (TempRepo, Arc<FakeGitHub>, Arc<NativeV2DeliveryAdapter>) {
    harness_with_grace(script, Duration::ZERO)
}

/// Three immediate polls, so a wait that outlasts them ends as a poll timeout.
fn harness_with_grace(
    script: Script,
    grace: Duration,
) -> (TempRepo, Arc<FakeGitHub>, Arc<NativeV2DeliveryAdapter>) {
    let (repo, authority) = delivery_harness(script);
    let adapter = retained_adapter(
        &repo,
        authority.clone(),
        DeliveryPollPolicy::new(3, Duration::ZERO)
            .assert_value()
            .with_ci_start_grace(grace),
        DeliveryLineage::original("target-drift"),
    );
    (repo, authority, adapter)
}

fn assert_still_waiting(outcome: &WorkerOutcome, authority: &FakeGitHub) {
    assert_eq!(
        outcome,
        &WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
    );
    assert_eq!(authority.inspections.load(Ordering::SeqCst), 3);
}

fn request(repo: &TempRepo) -> DeliveryRunRequest<'_> {
    DeliveryRunRequest {
        repo,
        attempts: 3,
        mode: MODE,
        run_id: "target-drift",
        refresh: None,
    }
}

/// Runs a pr@2 node whose declared contract accepts target drift.
pub(super) async fn run_accepting_target_drift(
    request: DeliveryRunRequest<'_>,
    adapter: Arc<NativeV2DeliveryAdapter>,
) -> DeliveryExecution {
    let node = delivery_node_for(request.mode, contract::TargetDrift::Accept);
    let admitted = NativeV2Admission
        .admit(delivery_submission_with_node(
            request.repo,
            request.mode,
            PullRequestFeedback::Consider,
            node,
        ))
        .await
        .assert_value();
    let handle = start_admitted(request, adapter, Some("test-token"), admitted).await;
    finish_execution(handle).await
}

fn response(mode: DeliveryMode, drift: contract::TargetDrift) -> NodeResponseContract {
    NodeResponseContract::Verifier {
        output: contract::delivery_result_schema_for(mode, drift).assert_value(),
        signals: BTreeMap::from([(
            FieldName::new(DELIVERY_SIGNAL_FIELD).assert_value(),
            contract::delivery_signal_labels_for(mode, drift).assert_value(),
        )]),
        diagnostic: delivery_diagnostic_schema().assert_value(),
    }
}

#[tokio::test]
async fn accepted_conflict_leaves_the_pull_request_open_without_repair() {
    let (repo, authority, adapter) = harness(Script::Conflict);

    let delivered = run_accepting_target_drift(request(&repo), adapter).await;

    let output = assert_delivery_signal(&delivered.outcome, DELIVERY_OPENED_LABEL);
    assert_receipt_match(output, MODE, &repo, true);
    assert_eq!(authority.review_requests().len(), 1);
    assert_eq!(authority.target_reconciliations.load(Ordering::SeqCst), 0);
    assert_eq!(
        authority.conflict_materializations.load(Ordering::SeqCst),
        0
    );
    assert_eq!(authority.head_updates.load(Ordering::SeqCst), 0);
    assert!(
        outcome_diagnostic(&delivered.outcome).contains("no required CI started within 0 seconds"),
        "{}",
        outcome_diagnostic(&delivered.outcome)
    );
    assert_eq!(
        git_output(&repo.workspace, &["status", "--porcelain=v1"]),
        "",
        "no conflict may be materialized in the workspace"
    );
}

#[tokio::test]
async fn accepted_conflict_waits_out_the_grace_period_for_ci_to_start() {
    let (repo, authority, adapter) =
        harness_with_grace(Script::Conflict, Duration::from_secs(3_600));

    let delivered = run_accepting_target_drift(request(&repo), adapter).await;

    assert_still_waiting(&delivered.outcome, &authority);
    assert_eq!(
        authority.conflict_materializations.load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn accepted_conflict_waits_for_started_ci_beyond_the_grace_period() {
    let (repo, authority, adapter) = harness(Script::ConflictCiRunning);

    let delivered = run_accepting_target_drift(request(&repo), adapter).await;

    assert_still_waiting(&delivered.outcome, &authority);
}

#[tokio::test]
async fn accepted_conflict_routes_failed_ci_to_repair() {
    // A started failure repairs at once, however much grace remains.
    let (repo, authority, adapter) =
        harness_with_grace(Script::ConflictCiFailed, Duration::from_secs(3_600));

    let delivered = run_accepting_target_drift(request(&repo), adapter).await;

    let output = assert_delivery_signal(&delivered.outcome, DELIVERY_CI_FAILED_LABEL);
    assert_receipt_match(output, MODE, &repo, false);
    assert_eq!(authority.inspections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn accepted_conflict_with_passed_ci_opens_without_waiting_for_grace() {
    let (repo, authority, adapter) =
        harness_with_grace(Script::ConflictCiPassed, Duration::from_secs(3_600));

    let delivered = run_accepting_target_drift(request(&repo), adapter).await;

    let output = assert_delivery_signal(&delivered.outcome, DELIVERY_OPENED_LABEL);
    assert_receipt_match(output, MODE, &repo, true);
    assert_eq!(authority.inspections.load(Ordering::SeqCst), 1);
    assert!(
        outcome_diagnostic(&delivered.outcome).contains("required CI passed"),
        "{}",
        outcome_diagnostic(&delivered.outcome)
    );
}

#[tokio::test]
async fn accepted_stale_head_is_not_updated() {
    let (repo, authority, adapter) = harness(Script::BehindPullRequest);

    let delivered = run_accepting_target_drift(request(&repo), adapter).await;

    let output = assert_delivery_signal(&delivered.outcome, DELIVERY_OPENED_LABEL);
    assert_receipt_match(output, MODE, &repo, true);
    assert_eq!(authority.head_updates.load(Ordering::SeqCst), 0);
    assert_eq!(authority.merge_requests.load(Ordering::SeqCst), 0);
    assert!(
        outcome_diagnostic(&delivered.outcome).contains("behind the target branch"),
        "{}",
        outcome_diagnostic(&delivered.outcome)
    );
}

#[tokio::test]
async fn reconciling_contract_still_routes_the_same_conflict() {
    let (repo, authority, adapter) = harness(Script::Conflict);

    let delivered = run_with_adapter(request(&repo), adapter).await;

    assert_delivery_signal(&delivered.outcome, DELIVERY_CONFLICT_LABEL);
    assert_eq!(
        authority.conflict_materializations.load(Ordering::SeqCst),
        1
    );
}

#[test]
fn only_the_current_pull_request_worker_accepts_target_drift() {
    let accept = contract::TargetDrift::Accept;
    let accepting = response(MODE, accept);
    assert!(validate_delivery_contract(MODE, &accepting).is_ok());
    assert_eq!(contract::target_drift(MODE, &accepting), accept);
    assert_eq!(
        contract::target_drift(MODE, &response(MODE, contract::TargetDrift::Reconcile)),
        contract::TargetDrift::Reconcile
    );
    for mode in [DeliveryMode::Merge, DeliveryMode::MergeV3] {
        // The accepting label set exists only for pr@2; a merge must integrate its target.
        let NodeResponseContract::Verifier {
            signals,
            diagnostic,
            ..
        } = accepting.clone()
        else {
            unreachable!()
        };
        let borrowed = NodeResponseContract::Verifier {
            output: delivery_result_schema(mode).assert_value(),
            signals,
            diagnostic,
        };
        assert!(validate_delivery_contract(mode, &borrowed).is_err());
    }
    // Signals and the output's outcome enum must select the same variant.
    let NodeResponseContract::Verifier {
        signals,
        diagnostic,
        ..
    } = accepting.clone()
    else {
        unreachable!()
    };
    for (mode, output) in [
        (MODE, delivery_result_schema(MODE)),
        (
            DeliveryMode::PullRequest,
            delivery_result_schema(DeliveryMode::PullRequest),
        ),
        (
            DeliveryMode::Push,
            delivery_result_schema(DeliveryMode::Push),
        ),
    ] {
        let mismatched = NodeResponseContract::Verifier {
            output: output.assert_value(),
            signals: signals.clone(),
            diagnostic: diagnostic.clone(),
        };
        assert!(
            validate_delivery_contract(mode, &mismatched).is_err(),
            "{mode:?}"
        );
    }
    // The legacy open-only worker declares `opened` without becoming drift-aware.
    let legacy = DeliveryMode::PullRequest;
    assert_eq!(
        contract::target_drift(legacy, &response(legacy, contract::TargetDrift::Reconcile)),
        contract::TargetDrift::Reconcile
    );
}

#[test]
fn opened_is_a_success_receipt_only_for_the_current_pull_request_worker() {
    assert!(MODE.is_success_outcome(DELIVERY_OPENED_LABEL));
    assert!(MODE.is_success_outcome(DELIVERY_READY_LABEL));
    assert!(!MODE.is_success_outcome(DELIVERY_CONFLICT_LABEL));
    assert!(!DeliveryMode::MergeV3.is_success_outcome(DELIVERY_OPENED_LABEL));
}
