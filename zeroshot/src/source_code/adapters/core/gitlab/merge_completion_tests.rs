use super::*;

pub(super) fn require_merge_base_refs(state: &StateData, uri: &reqwest::Url) {
    let refs = uri
        .query_pairs()
        .filter(|(key, _)| key == "refs[]")
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    let target = &state.responses[&("GET".into(), "/repository/branches/main".into())].1;
    assert_eq!(refs, [HEAD, target["commit"]["id"].as_str().unwrap()]);
}

fn merged_fast_forward(fixture: &Fixture, target: &str) {
    let mut review = merge_request();
    review["state"] = json!("merged");
    {
        let mut state = fixture.state.lock().unwrap();
        state.project["merge_method"] = json!("ff");
        state.review = Some(review);
    }
    fixture.response(
        "GET",
        "/repository/branches/main",
        (
            StatusCode::OK,
            json!({"name":"main","commit":{"id":target}}),
        ),
    );
}

async fn assert_completion(fixture: &Fixture) {
    let expected = SourceReviewState::Merged {
        merge_revision: HEAD.into(),
    };
    for known in [false, true] {
        assert_eq!(
            fixture
                .observe_review(known)
                .await
                .unwrap()
                .review
                .unwrap()
                .state,
            expected
        );
    }
    assert_eq!(
        fixture
            .authority
            .inspect_review(&receipt(), credential())
            .await
            .unwrap()
            .state,
        expected
    );
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap(),
        SourceMergeRequestOutcome::Accepted
    );
    assert_eq!(
        fixture
            .authority
            .withdraw_merge_request(&receipt(), credential())
            .await
            .unwrap(),
        SourceMergeWithdrawal::Merged {
            merge_revision: HEAD.into()
        }
    );
}

#[tokio::test]
async fn fast_forward_without_merge_shas_confirms_completion_and_lost_response() {
    for lost in [false, true] {
        let fixture = Fixture::new().await;
        fixture.open_review();
        {
            let mut state = fixture.state.lock().unwrap();
            state.project["merge_method"] = json!("ff");
            state.lose_merge = lost;
        }
        let result = fixture
            .authority
            .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
            .await;
        if lost {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), SourceMergeRequestOutcome::Accepted);
            assert!(fixture.authority.effects.lock().unwrap().merges.is_empty());
        }
        // GitLab can remove the source branch as part of the successful merge.
        fixture.response(
            "GET",
            "/repository/branches/zeroshot%2Ftest",
            (StatusCode::NOT_FOUND, Value::Null),
        );
        assert_completion(&fixture).await;
        assert_eq!(fixture.writes("/merge"), 1);
    }
}

#[tokio::test]
async fn fast_forward_completion_proves_ancestry_after_the_target_advances_again() {
    let fixture = Fixture::new().await;
    merged_fast_forward(&fixture, MERGED);
    fixture.response(
        "GET",
        "/repository/merge_base",
        (StatusCode::OK, json!({"id":HEAD})),
    );
    assert_completion(&fixture).await;
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn fast_forward_completion_requires_target_and_ancestry_evidence() {
    for (status, body) in [
        (StatusCode::OK, json!({"id":BASE})),
        (StatusCode::OK, json!({"id":"invalid"})),
        (StatusCode::OK, json!({"id":null})),
        (StatusCode::NOT_FOUND, Value::Null),
        (StatusCode::SERVICE_UNAVAILABLE, Value::Null),
    ] {
        let fixture = Fixture::new().await;
        merged_fast_forward(&fixture, MERGED);
        fixture.response("GET", "/repository/merge_base", (status, body));
        assert!(
            fixture
                .authority
                .inspect_review(&receipt(), credential())
                .await
                .is_err()
        );
        assert!(
            fixture
                .authority
                .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .is_err()
        );
        assert!(
            fixture
                .authority
                .withdraw_merge_request(&receipt(), credential())
                .await
                .is_err()
        );
        assert!(fixture.observe_review(true).await.is_err());
        assert_eq!(fixture.writes("/merge"), 0);
    }
    for (status, branch) in [
        (StatusCode::NOT_FOUND, Value::Null),
        (StatusCode::OK, json!({"name":"other","commit":{"id":HEAD}})),
        (
            StatusCode::OK,
            json!({"name":"main","commit":{"id":"invalid"}}),
        ),
    ] {
        let fixture = Fixture::new().await;
        merged_fast_forward(&fixture, HEAD);
        fixture.response("GET", "/repository/branches/main", (status, branch));
        assert!(
            fixture
                .authority
                .inspect_review(&receipt(), credential())
                .await
                .is_err()
        );
        assert_eq!(fixture.writes("/merge"), 0);
    }
}

#[tokio::test]
async fn target_containment_never_substitutes_for_other_merge_methods_or_invalid_merge_shas() {
    for method in ["merge", "rebase_merge"] {
        let fixture = Fixture::new().await;
        merged_fast_forward(&fixture, HEAD);
        fixture.state.lock().unwrap().project["merge_method"] = json!(method);
        assert!(
            fixture
                .authority
                .inspect_review(&receipt(), credential())
                .await
                .is_err()
        );
    }
    for field in ["merge_commit_sha", "squash_commit_sha"] {
        let fixture = Fixture::new().await;
        merged_fast_forward(&fixture, HEAD);
        fixture.change_review(field, json!("invalid"));
        assert!(
            fixture
                .authority
                .inspect_review(&receipt(), credential())
                .await
                .is_err()
        );
        fixture.change_review(field, json!(MERGED));
        assert_eq!(
            fixture
                .authority
                .inspect_review(&receipt(), credential())
                .await
                .unwrap()
                .state,
            SourceReviewState::Merged {
                merge_revision: MERGED.into()
            }
        );
    }
}

#[tokio::test]
async fn synchronization_rereads_the_old_mr_head_after_publishing_an_integration() {
    for known in [false, true] {
        let fixture = Fixture::new().await;
        fixture.open_review();
        fixture.move_branch_to(MERGED);
        let mut updated = merge_request();
        updated["sha"] = json!(MERGED);
        updated["title"] = json!("Old metadata");
        fixture.state.lock().unwrap().review_sequence =
            vec![merge_request(), merge_request(), updated];
        let mut request = request();
        request.head_revision = MERGED.into();
        request.known_review = known.then(receipt);
        let receipt = fixture
            .authority
            .open_or_update_review(&request, credential())
            .await
            .unwrap();
        assert_eq!(receipt.head_revision, MERGED);
        assert!(fixture.state.lock().unwrap().review_sequence.is_empty());
        assert_eq!(fixture.writes("/merge_requests"), 0);
        assert_eq!(fixture.writes("/merge_requests/17"), 1);
        assert_eq!(fixture.writes("/merge"), 0);
    }
}

#[tokio::test]
async fn synchronization_refuses_persistent_lag_and_a_consistent_foreign_head_without_writes() {
    for (known, branch) in [(false, MERGED), (true, MERGED), (true, HEAD)] {
        let fixture = Fixture::new().await;
        fixture.open_review();
        fixture.move_branch_to(branch);
        let mut request = request();
        request.head_revision = MERGED.into();
        request.known_review = known.then(receipt);
        let error = fixture
            .authority
            .open_or_update_review(&request, credential())
            .await
            .unwrap_err();
        assert!(
            matches!(error, SourceDeliveryError::Identity(_)),
            "{error:?}"
        );
        assert!(!error.retryable_operation());
        assert_eq!(fixture.writes("/merge_requests"), 0);
        assert_eq!(fixture.writes("/merge_requests/17"), 0);
        assert_eq!(fixture.writes("/merge"), 0);
    }
}
