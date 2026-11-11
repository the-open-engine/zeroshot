use super::*;

#[tokio::test]
async fn incomplete_bounded_discovery_never_authorizes_review_creation() {
    let fixture = Fixture::new().await;
    let mut closed = merge_request();
    closed["state"] = json!("closed");
    fixture.response(
        "GET",
        "/merge_requests",
        (StatusCode::OK, json!(vec![closed; 100])),
    );
    let mut request = request();
    request.include_terminal_reviews = false;
    let error = fixture
        .authority
        .open_or_update_review(&request, credential())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("complete-read bound"));
    assert_eq!(fixture.writes("/merge_requests"), 0);
}

#[tokio::test]
async fn reopened_review_behind_a_full_page_of_closed_history_is_reused() {
    let fixture = Fixture::new().await;
    {
        let mut state = fixture.state.lock().unwrap();
        state.historical_reviews = (100..200)
            .map(|iid| {
                let mut review = merge_request();
                review["iid"] = json!(iid);
                review["state"] = json!("closed");
                review
            })
            .collect();
        state.review = Some(merge_request());
    }
    let mut request = request();
    request.include_terminal_reviews = false;
    let snapshot = fixture
        .authority
        .observe_delivery(
            SourceDeliveryRead {
                target: &request.target,
                head_branch: &request.head_branch,
                known_review: None,
                include_review: true,
                include_terminal_reviews: false,
            },
            credential(),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.review.unwrap().review_id, "17");
    assert_eq!(
        fixture
            .authority
            .open_or_update_review(&request, credential())
            .await
            .unwrap()
            .review_id,
        "17"
    );
    assert_eq!(fixture.writes("/merge_requests"), 0);
    fixture.state.lock().unwrap().review.as_mut().unwrap()["iid"] = json!(18);
    let mut second_active = merge_request();
    second_active["iid"] = json!(19);
    fixture
        .state
        .lock()
        .unwrap()
        .historical_reviews
        .push(second_active);
    assert!(
        fixture
            .authority
            .open_or_update_review(&request, credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/merge_requests"), 0);
}

#[tokio::test]
async fn fresh_discovery_creation_and_lost_response_recovery_ignore_historical_reviews() {
    for active in [false, true] {
        let fixture = Fixture::new().await;
        let mut closed = merge_request();
        closed["iid"] = json!(1);
        closed["state"] = json!("closed");
        let mut merged = closed.clone();
        merged["iid"] = json!(2);
        merged["state"] = json!("merged");
        merged["merge_commit_sha"] = json!(MERGED);
        {
            let mut state = fixture.state.lock().unwrap();
            state.historical_reviews = vec![closed, merged];
            state.review = active.then(merge_request);
            state.lose_create = !active;
        }
        let mut request = request();
        request.include_terminal_reviews = false;
        let snapshot = fixture
            .authority
            .observe_delivery(
                SourceDeliveryRead {
                    target: &request.target,
                    head_branch: &request.head_branch,
                    known_review: None,
                    include_review: true,
                    include_terminal_reviews: false,
                },
                credential(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot.review.is_some(), active);
        let receipt = fixture
            .authority
            .open_or_update_review(&request, credential())
            .await
            .unwrap();
        assert_eq!(receipt.review_id, "17");
        assert_eq!(fixture.writes("/merge_requests"), usize::from(!active));

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
        let resumed = fixture
            .authority
            .observe_delivery(
                SourceDeliveryRead {
                    target: &request.target,
                    head_branch: &request.head_branch,
                    known_review: request.known_review.as_ref(),
                    include_review: true,
                    include_terminal_reviews: true,
                },
                credential(),
            )
            .await
            .unwrap();
        assert_eq!(resumed.review.unwrap().review_id, receipt.review_id);
        fixture
            .authority
            .open_or_update_review(&request, credential())
            .await
            .unwrap();
        assert_eq!(fixture.writes("/merge_requests"), usize::from(!active));
        fixture.state.lock().unwrap().review.as_mut().unwrap()["state"] = json!("closed");
        let snapshot = fixture
            .authority
            .observe_delivery(
                SourceDeliveryRead {
                    target: &request.target,
                    head_branch: &request.head_branch,
                    known_review: Some(&receipt),
                    include_review: true,
                    include_terminal_reviews: true,
                },
                credential(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot.review.unwrap().state, SourceReviewState::Closed);
        assert!(
            fixture
                .authority
                .open_or_update_review(&request, credential())
                .await
                .is_err()
        );
    }
}
