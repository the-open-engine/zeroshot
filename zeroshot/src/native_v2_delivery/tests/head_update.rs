use super::*;

#[tokio::test]
async fn strict_freshness_advances_the_exact_head_before_merge() {
    assert_head_updates(Script::StrictBehind, 1, 2, 4).await;
}

#[tokio::test]
async fn repeated_base_advances_form_an_authorized_head_chain() {
    assert_head_updates(Script::RepeatedBehind, 2, 3, 5).await;
}

#[tokio::test]
async fn provider_receipt_survives_a_transient_local_adoption_failure() {
    let (repo, authority) = delivery_harness(Script::HeadAdoptionRace);

    let outcome = run_delivery(&repo, authority.clone(), 3, DeliveryMode::Merge).await;

    assert_delivery_signal(&outcome, DELIVERY_MERGED_LABEL);
    assert_eq!(authority.head_updates.load(Ordering::SeqCst), 1);
    assert_eq!(authority.head_sync_attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn permanent_local_adoption_rejection_fails_closed_without_retrying_cas() {
    let (repo, authority) = delivery_harness(Script::HeadAdoptionRejected);

    let outcome = run_delivery(&repo, authority.clone(), 3, DeliveryMode::Merge).await;

    assert_eq!(
        outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
    );
    assert_eq!(authority.head_updates.load(Ordering::SeqCst), 1);
    assert_eq!(authority.head_sync_attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn repeated_local_adoption_unavailability_is_bounded_without_retrying_cas() {
    let (repo, authority) = delivery_harness(Script::HeadAdoptionUnavailable);

    let outcome = run_delivery(&repo, authority.clone(), 3, DeliveryMode::Merge).await;

    assert_eq!(
        outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
    );
    assert_eq!(authority.head_updates.load(Ordering::SeqCst), 1);
    assert_eq!(authority.head_sync_attempts.load(Ordering::SeqCst), 5);
}

async fn assert_head_updates(
    script: Script,
    expected_updates: usize,
    expected_merge_requests: usize,
    attempts: usize,
) {
    let (repo, authority) = delivery_harness(script);

    let outcome = run_delivery(&repo, authority.clone(), attempts, DeliveryMode::Merge).await;

    let output = assert_delivery_signal(&outcome, DELIVERY_MERGED_LABEL);
    assert_receipt_match(output, DeliveryMode::Merge, &repo, true);
    assert_eq!(
        authority.head_updates.load(Ordering::SeqCst),
        expected_updates
    );
    assert_eq!(
        authority.merge_requests.load(Ordering::SeqCst),
        expected_merge_requests
    );
    let local_head = std::process::Command::new("/usr/bin/git")
        .arg("-C")
        .arg(&repo.workspace)
        .args(["rev-parse", "HEAD"])
        .output()
        .assert_value();
    assert_eq!(
        output
            .pointer("/headRevision")
            .and_then(Value::as_str)
            .assert_value(),
        String::from_utf8(local_head.stdout).assert_value().trim()
    );
}
