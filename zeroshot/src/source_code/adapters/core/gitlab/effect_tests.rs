use crate::source_code_provider::TestReviewCheckpoint;
use super::*;

#[tokio::test]
async fn missing_source_branch_can_create_after_rechecking_the_published_head() {
    let fixture = Fixture::new().await;
    fixture.response(
        "POST",
        "/merge_requests",
        (
            StatusCode::BAD_REQUEST,
            json!({"message":["source_branch does not exist"]}),
        ),
    );
    let error = fixture
        .authority
        .open_or_update_review(&request(), credential())
        .await
        .unwrap_err();
    assert!(error.retryable_review_sync(), "{error}");
    fixture.state.lock().unwrap().responses.clear();
    let observed = fixture
        .authority
        .open_or_update_review(&request(), credential())
        .await
        .unwrap();
    assert_eq!(observed, receipt());
    assert_eq!(fixture.writes("/merge_requests"), 2);
    let state = fixture.state.lock().unwrap();
    let calls: Vec<_> = state
        .calls
        .iter()
        .filter_map(|(method, path, _)| {
            if method == "POST" {
                Some("create")
            } else if path.ends_with("/repository/branches/zeroshot%2Ftest") {
                Some("branch")
            } else if path.ends_with("/merge_requests") {
                Some("find")
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        calls,
        [
            "find", "branch", "create", "find", "find", "branch", "create"
        ]
    );
}

#[tokio::test]
async fn missing_source_branch_retry_refuses_a_changed_or_missing_published_head() {
    for revision in [Some(MERGED), None] {
        let fixture = Fixture::new().await;
        fixture.response(
            "POST",
            "/merge_requests",
            (
                StatusCode::BAD_REQUEST,
                json!({"message":["source_branch does not exist"]}),
            ),
        );
        assert!(
            fixture
                .authority
                .open_or_update_review(&request(), credential())
                .await
                .unwrap_err()
                .retryable_review_sync()
        );
        fixture.state.lock().unwrap().responses.clear();
        match revision {
            Some(revision) => fixture.move_branch_to(revision),
            None => fixture.response(
                "GET",
                "/repository/branches/zeroshot%2Ftest",
                (StatusCode::NOT_FOUND, Value::Null),
            ),
        }
        let error = fixture
            .authority
            .open_or_update_review(&request(), credential())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("not the published candidate"),
            "{error}"
        );
        assert!(!error.retryable_review_sync());
        assert_eq!(fixture.writes("/merge_requests"), 1);
    }
}

#[tokio::test]
async fn explicit_create_rejection_releases_the_fence_but_ambiguous_failure_does_not() {
    let fixture = Fixture::new().await;
    fixture.response(
        "POST",
        "/merge_requests",
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"message":"policy"}),
        ),
    );
    assert_eq!(
        fixture
            .authority
            .open_or_update_review(&request(), credential())
            .await
            .unwrap_err()
            .api_status(),
        Some(422)
    );
    fixture.response(
        "POST",
        "/merge_requests",
        (StatusCode::SERVICE_UNAVAILABLE, Value::Null),
    );
    assert!(
        fixture
            .authority
            .open_or_update_review(&request(), credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/merge_requests"), 2);
    let error = fixture
        .authority
        .open_or_update_review(&request(), credential())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("creation remains uncertain"));
    assert_eq!(fixture.writes("/merge_requests"), 2);
}

#[tokio::test]
async fn server_errors_and_lost_creation_responses_never_authorize_another_post() {
    for status in [
        Some(StatusCode::INTERNAL_SERVER_ERROR),
        Some(StatusCode::SERVICE_UNAVAILABLE),
        None,
    ] {
        let fixture = Fixture::new().await;
        if let Some(status) = status {
            fixture.response(
                "POST",
                "/merge_requests",
                (status, json!({"message":["source_branch does not exist"]})),
            );
        } else {
            fixture.state.lock().unwrap().disconnect_create = true;
        }
        let error = fixture
            .authority
            .open_or_update_review(&request(), credential())
            .await
            .unwrap_err();
        assert!(!error.retryable_review_sync(), "{error}");
        assert!(!error.retryable_operation(), "{error}");
        assert!(
            error.to_string().contains("creation response was lost"),
            "{error}"
        );
        fixture.state.lock().unwrap().responses.clear();
        fixture.state.lock().unwrap().disconnect_create = false;
        let error = fixture
            .authority
            .open_or_update_review(&request(), credential())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("creation remains uncertain"),
            "{error}"
        );
        assert_eq!(fixture.writes("/merge_requests"), 1);
    }
}

#[tokio::test]
async fn explicit_merge_rejection_can_retry_while_an_ambiguous_response_stops_effects() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.response(
        "PUT",
        "/merge",
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"message":"policy"}),
        ),
    );
    for _ in 0..2 {
        assert_eq!(
            fixture
                .authority
                .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap_err()
                .api_status(),
            Some(422)
        );
    }
    fixture.response(
        "PUT",
        "/merge",
        (StatusCode::SERVICE_UNAVAILABLE, Value::Null),
    );
    assert!(
        fixture
            .authority
            .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/merge"), 3);
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap(),
        SourceMergeRequestOutcome::Pending
    );
    assert!(
        fixture
            .authority
            .withdraw_merge_request(&receipt(), credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/merge"), 3);
}

#[tokio::test]
async fn accepted_but_unconfirmed_merge_retains_its_effect_fence() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.response("PUT", "/merge", (StatusCode::OK, merge_request()));
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
            .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap(),
        SourceMergeRequestOutcome::Pending
    );
    assert!(
        fixture
            .authority
            .withdraw_merge_request(&receipt(), credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/merge"), 1);
}

#[tokio::test]
async fn duplicate_reviews_and_unpublished_candidate_heads_stop_before_creation() {
    let fixture = Fixture::new().await;
    fixture.response(
        "GET",
        "/merge_requests",
        (StatusCode::OK, json!([merge_request(), merge_request()])),
    );
    assert!(
        fixture
            .authority
            .open_or_update_review(&request(), credential())
            .await
            .unwrap_err()
            .to_string()
            .contains("Multiple GitLab merge requests")
    );
    fixture.response("GET", "/merge_requests", (StatusCode::OK, json!([])));
    fixture.response(
        "GET",
        "/repository/branches/zeroshot%2Ftest",
        (StatusCode::NOT_FOUND, Value::Null),
    );
    assert!(
        fixture
            .authority
            .open_or_update_review(&request(), credential())
            .await
            .unwrap_err()
            .to_string()
            .contains("not the published candidate")
    );
    assert_eq!(fixture.writes("/merge_requests"), 0);
}

#[tokio::test]
async fn known_review_and_branch_observation_are_fenced_against_head_movement() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    let request = request();
    let known = receipt();
    let read = SourceDeliveryRead {
        target: &request.target,
        head_branch: &request.head_branch,
        known_review: Some(&known),
        include_review: true,
    };
    let observed = fixture
        .authority
        .observe_delivery(read, credential())
        .await
        .unwrap();
    assert!(observed.review.is_some());
    fixture.response(
        "GET",
        "/repository/branches/zeroshot%2Ftest",
        (
            StatusCode::OK,
            json!({"name":"zeroshot/test","commit":{"id":MERGED}}),
        ),
    );
    assert!(
        fixture
            .authority
            .observe_delivery(read, credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn incomplete_or_reordered_generated_sections_are_never_overwritten() {
    let fixture = Fixture::new().await;
    for description in [
        "Human report without a generated section.".to_owned(),
        format!("{DELIVERY_GENERATED_BODY_START}\nMissing end"),
        format!("{DELIVERY_GENERATED_BODY_END}\n{DELIVERY_GENERATED_BODY_START}"),
        format!(
            "{DELIVERY_GENERATED_BODY_START}\nReport\n{DELIVERY_GENERATED_BODY_END}\n{DELIVERY_GENERATED_BODY_END}"
        ),
    ] {
        let mut mr = merge_request();
        mr["description"] = json!(description);
        fixture.state.lock().unwrap().review = Some(mr);
        assert!(
            fixture
                .authority
                .open_or_update_review(&request(), credential())
                .await
                .is_err()
        );
    }
    assert_eq!(fixture.writes("/merge_requests/17"), 0);
    fixture.open_review();
    let mut altered = merge_request();
    altered["title"] = json!("Server substituted title");
    fixture.response("PUT", "/merge_requests/17", (StatusCode::OK, altered));
    let mut next = request();
    next.title = "Updated title".into();
    assert!(
        fixture
            .authority
            .open_or_update_review(&next, credential())
            .await
            .unwrap_err()
            .to_string()
            .contains("did not confirm")
    );
    assert_eq!(fixture.writes("/merge_requests/17"), 1);
}

#[tokio::test]
async fn malformed_project_or_review_identity_cannot_cross_the_effect_boundary() {
    let fixture = Fixture::new().await;
    for (field, value) in [
        ("id", json!(0)),
        ("web_url", json!("https://foreign.example/project")),
        ("merge_method", json!("future-policy")),
        ("path_with_namespace", json!("other/project")),
    ] {
        let mut observed = project();
        observed[field] = value;
        fixture.state.lock().unwrap().project = observed;
        assert!(
            fixture
                .authority
                .open_or_update_review(&request(), credential())
                .await
                .is_err()
        );
    }
    fixture.state.lock().unwrap().project = project();
    for (field, value) in [
        ("id", json!(0)),
        ("iid", json!(0)),
        ("target_project_id", json!(22)),
        ("source_project_id", json!(22)),
        ("source_branch", json!("other")),
        ("target_branch", json!("other")),
        ("sha", json!(123)),
    ] {
        let mut observed = merge_request();
        observed[field] = value;
        fixture.state.lock().unwrap().review = Some(observed);
        assert!(
            fixture
                .authority
                .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .is_err()
        );
    }
    assert_eq!(fixture.writes("/merge_requests"), 0);
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn feedback_ignores_system_and_empty_notes_but_retains_position_and_edit_identity() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    for (body, system) in [("System event", true), (" \n\t", false)] {
        let mut current = note(1, body);
        current["system"] = json!(system);
        fixture.state.lock().unwrap().discussion_note = current;
        assert!(
            fixture
                .authority
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap()
                .items
                .is_empty()
        );
    }
    let mut current = note(1, "Review this line.");
    current["position"] = json!({"new_path":"src/main.rs","new_line":3});
    fixture.state.lock().unwrap().discussion_note = current;
    let first = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap()
        .items
        .remove(0);
    assert_eq!(
        first.location.as_deref(),
        Some(r#"{"new_line":3,"new_path":"src/main.rs"}"#)
    );
    fixture.state.lock().unwrap().discussion_note["resolved"] = json!(true);
    let edited = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap()
        .items
        .remove(0);
    assert_eq!(first.key, edited.key);
    assert_ne!(first.version, edited.version);
}
