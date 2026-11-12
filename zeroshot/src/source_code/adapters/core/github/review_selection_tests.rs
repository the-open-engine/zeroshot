use super::*;

#[tokio::test]
async fn fresh_discovery_and_creation_ignore_history_and_refresh_a_bound_review_by_identity() {
    for active in [false, true] {
        let root = tempfile::tempdir().assert_value();
        let mut request = test_review_request();
        request.include_terminal_reviews = false;
        let body = pull_request_body(&request).assert_value();
        let mut exact = review_wire(&request, Some(&request.title), Some(&body));
        exact["state"] = json!("open");
        exact["merged"] = json!(false);
        let mut closed = exact.clone();
        closed["number"] = json!(1);
        closed["state"] = json!("closed");
        let mut merged = closed.clone();
        merged["number"] = json!(2);
        merged["merged"] = json!(true);
        let authority = review_authority(
            root.path(),
            uniform_review_responses(json!([closed, merged]), &exact),
        );
        std::fs::write(
            root.path().join("active-reviews.json"),
            if active { json!([exact]) } else { json!([]) }.to_string(),
        )
        .assert_value();
        let snapshot = authority
            .observe_delivery(
                SourceDeliveryRead {
                    target: &request.target,
                    head_branch: &request.head_branch,
                    known_review: None,
                    include_review: true,
                    include_terminal_reviews: false,
                },
                SourceDeliveryCredential::token("test-token"),
            )
            .await
            .assert_value();
        assert_eq!(snapshot.review.is_some(), active);
        let receipt = authority
            .open_or_update_review(&request, SourceDeliveryCredential::token("test-token"))
            .await
            .assert_value();
        assert_eq!(receipt.review_id, "17");
        let posts = usize::from(!active);
        let calls = || std::fs::read_to_string(root.path().join("review-calls")).assert_value();
        assert_eq!(calls().matches("pulls --method POST").count(), posts);

        // Branch history must not replace the review this run has already bound.
        request.include_terminal_reviews = true;
        request.known_review = crate::native_v2_delivery::contract::review_from_receipt(
            &json!({"version":"v1", "mode":"pr", "outcome":"opened",
                "repository":receipt.repository, "targetBranch":receipt.target_branch,
                "headRevision":receipt.head_revision, "pullRequestId":receipt.review_id}),
            crate::native_v2_delivery::DeliveryMode::PullRequest,
            &request.target,
            &request.head_branch,
        );
        assert_eq!(request.known_review.as_ref(), Some(&receipt));
        std::fs::write(root.path().join("active-reviews.json"), "[]").assert_value();
        let resumed = authority
            .observe_delivery(
                SourceDeliveryRead {
                    target: &request.target,
                    head_branch: &request.head_branch,
                    known_review: request.known_review.as_ref(),
                    include_review: true,
                    include_terminal_reviews: true,
                },
                SourceDeliveryCredential::token("test-token"),
            )
            .await
            .assert_value();
        assert_eq!(resumed.review.assert_value().review_id, receipt.review_id);
        assert_eq!(
            authority
                .open_or_update_review(&request, SourceDeliveryCredential::token("test-token"))
                .await
                .assert_value(),
            receipt
        );
        assert_eq!(calls().matches("pulls --method POST").count(), posts);
        let mut terminal = review_wire(&request, Some(&request.title), Some(&body));
        terminal["state"] = json!("closed");
        terminal["merged"] = json!(false);
        write_review_fixture(root.path(), uniform_review_responses(json!([]), &terminal));
        let closed = authority
            .observe_delivery(
                SourceDeliveryRead {
                    target: &request.target,
                    head_branch: &request.head_branch,
                    known_review: request.known_review.as_ref(),
                    include_review: true,
                    include_terminal_reviews: true,
                },
                SourceDeliveryCredential::token("test-token"),
            )
            .await
            .assert_value();
        assert_eq!(
            closed.review.assert_value().state,
            SourceReviewState::Closed
        );
    }
}
