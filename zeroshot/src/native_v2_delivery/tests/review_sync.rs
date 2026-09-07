use super::*;

#[tokio::test]
async fn pushed_review_head_is_retried_for_pr_and_ship_during_github_visibility_lag() {
    for (mode, outcome) in [
        (DeliveryMode::PullRequest, DELIVERY_OPENED_LABEL),
        (DeliveryMode::Merge, DELIVERY_MERGED_LABEL),
    ] {
        let repo = TempRepo::delivery();
        let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::ReviewSyncRace));

        let delivery = run_delivery(&repo, authority.clone(), 3, mode).await;

        assert_delivery_signal(&delivery, outcome);
        assert_eq!(authority.review_sync_attempts.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn review_sync_refreshes_an_expired_dynamic_github_credential() {
    let repo = TempRepo::delivery();
    let authority = Arc::new(FakeGitHub::new(
        repo.remote.clone(),
        Script::ReviewSyncCredentialExpires,
    ));
    let binding = delivery_runtime_binding(&repo, DeliveryMode::PullRequest).await;

    let outcome = run_delivery_with_id(
        DeliveryRunRequest {
            repo: &repo,
            attempts: 2,
            mode: DeliveryMode::PullRequest,
            run_id: "refresh-review-sync-credential",
            refresh: Some(Arc::new(RefreshedDeliveryEnvironment { binding })),
        },
        authority.clone(),
    )
    .await;

    assert_delivery_signal(&outcome, DELIVERY_OPENED_LABEL);
    assert_eq!(authority.review_sync_attempts.load(Ordering::SeqCst), 2);
}
