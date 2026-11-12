use crate::source_code_provider::TestReviewCheckpoint;
use super::*;
use crate::source_code_provider::adapters::core::review_responses;

pub(super) fn conversation_response(
    state: &mut StateData,
    method: &Method,
    path: &str,
    body: Value,
) -> Response {
    if method == Method::GET
        && path.ends_with("/discussions/thread")
        && state.review.as_ref().unwrap()["blocking_discussions_resolved"] == true
        && state.confirmation_failures > 0
    {
        state.confirmation_failures -= 1;
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"message":"Confirmation unavailable"})),
        )
            .into_response();
    }
    let thread = state.conversation.as_mut().unwrap();
    if path.ends_with("/discussions") {
        return Json(json!([thread])).into_response();
    }
    if method == Method::POST {
        if state.conversation_race.as_deref() == Some("reject_reply") {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message":"Reply unavailable"})),
            )
                .into_response();
        }
        let next_id = thread["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|note| note["id"].as_u64().unwrap())
            .max()
            .unwrap_or(500)
            .max(500)
            + 1;
        let mut reply = note(next_id, body["body"].as_str().unwrap());
        reply["resolvable"] = json!(true);
        reply["resolved"] = json!(false);
        thread["notes"].as_array_mut().unwrap().push(reply.clone());
        if state.conversation_race.as_deref() == Some("edit_after_reply") {
            thread["notes"][0]["body"] = json!("Edited finding.");
        }
        if std::mem::take(&mut state.lose_reply_response) {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message":"Reply acknowledgement lost"})),
            )
                .into_response();
        }
        return Json(reply).into_response();
    }
    if method == Method::PUT {
        let resolved = body["resolved"].as_bool().unwrap();
        for note in thread["notes"].as_array_mut().unwrap() {
            note["resolved"] = json!(resolved);
            note["updated_at"] = json!("resolution-control-update");
        }
        state.review.as_mut().unwrap()["blocking_discussions_resolved"] = json!(resolved);
        state.review.as_mut().unwrap()["detailed_merge_status"] = json!(if resolved {
            "mergeable"
        } else {
            "discussions_not_resolved"
        });
        if resolved && state.conversation_race.as_deref() == Some("edit_after_resolve") {
            thread["notes"][0]["body"] = json!("Another finding during resolution.");
            state.conversation_race = None;
        }
        if std::mem::take(if resolved {
            &mut state.lose_resolve_response
        } else {
            &mut state.lose_reopen_response
        }) {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message":"Acknowledgement lost"})),
            )
                .into_response();
        }
    }
    Json(thread.clone()).into_response()
}

fn prepare(fixture: &Fixture) {
    fixture.open_review();
    let mut data = fixture.state.lock().unwrap();
    let mut root = note(1, "Handle empty input.");
    root["resolvable"] = json!(true);
    root["resolved"] = json!(false);
    data.conversation = Some(json!({"id":"thread","notes":[root]}));
    data.review.as_mut().unwrap()["blocking_discussions_resolved"] = json!(false);
    data.review.as_mut().unwrap()["detailed_merge_status"] = json!("discussions_not_resolved");
}

fn response(feedback: &SourceReviewFeedback) -> SourceReviewResponse {
    SourceReviewResponse {
        thread_id: "thread".into(),
        version: feedback.conversations[0].version.clone(),
        disposition: SourceReviewDisposition::Addressed,
        response: "Guarded empty input.\n/close".into(),
    }
}

#[tokio::test]
async fn gitlab_large_verified_responses_retain_compact_receipts_across_repair_rounds() {
    let fixture = Fixture::new().await;
    prepare(&fixture);
    for round in 0..10 {
        if round > 0 {
            fixture.state.lock().unwrap().conversation.as_mut().unwrap()["notes"][0]["body"] =
                json!(format!("Later finding {round}."));
        }
        let feedback = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        let mut response = response(&feedback);
        response.response = "x".repeat(60_000);
        assert_eq!(
            fixture
                .authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &response,
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential()
                )
                .await
                .unwrap(),
            SourceReviewResolution::Resolved
        );
    }
    let checkpoint = fixture
        .authority
        .review_conversation_state()
        .await
        .unwrap()
        .unwrap();
    assert!(serde_json::to_vec(&checkpoint).unwrap().len() < 10_000);
    review_responses::assert_fits_ledger(checkpoint.clone());
    let mut recovered =
        GitLabDeliveryAuthority::new(fixture.authority.base_url.clone(), None).unwrap();
    recovered.api = fixture.authority.api.clone();
    recovered
        .restore_review_conversation_state(checkpoint)
        .await
        .unwrap();
    assert!(
        recovered
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    fixture.state.lock().unwrap().conversation.as_mut().unwrap()["notes"][0]["body"] =
        json!("Another substantive edit.");
    assert_eq!(
        recovered
            .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap(),
        SourceMergeRequestOutcome::ReviewBlocked
    );
    assert_eq!(fixture.writes("/merge"), 0);
    assert_eq!(fixture.writes("/discussions/thread/notes"), 10);
}

#[tokio::test]
async fn gitlab_accumulated_receipt_limit_rejects_before_posting_or_resolving() {
    let fixture = Fixture::new().await;
    prepare(&fixture);
    let checkpoint = review_responses::fill_receipt_storage(
        fixture
            .authority
            .review_conversation_state()
            .await
            .unwrap()
            .unwrap(),
        &receipt(),
    );
    fixture
        .authority
        .restore_review_conversation_state(checkpoint)
        .await
        .unwrap();
    let retained = fixture
        .authority
        .review_conversation_state()
        .await
        .unwrap()
        .unwrap();
    review_responses::assert_fits_ledger(retained.clone());
    let feedback = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap();
    assert!(
        fixture
            .authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response(&feedback),
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential()
            )
            .await
            .is_err()
    );
    assert_eq!(
        fixture
            .authority
            .review_conversation_state()
            .await
            .unwrap()
            .unwrap(),
        retained
    );
    assert_eq!(fixture.writes("/discussions/thread/notes"), 0);
    assert_eq!(fixture.writes("/discussions/thread"), 0);
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn gitlab_replayed_response_after_reconstruction_reopens_unproven_resolved_revision() {
    for changed in [false, true] {
        let fixture = Fixture::new().await;
        prepare(&fixture);
        let before = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        let expected = response(&before);
        {
            let mut data = fixture.state.lock().unwrap();
            data.confirmation_failures = 1;
            if changed {
                data.conversation_race = Some("edit_after_resolve".into());
            }
        }
        assert!(
            fixture
                .authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &expected,
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential()
                )
                .await
                .is_err()
        );
        let mut recovered =
            GitLabDeliveryAuthority::new(SourceForgeBaseUrl::new(ORIGIN).unwrap(), None).unwrap();
        recovered.api = fixture.authority.api.clone();
        assert_eq!(
            recovered
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &expected,
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential()
                )
                .await
                .unwrap(),
            SourceReviewResolution::Changed
        );
        let after = recovered
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        assert_eq!(after.conversations.len(), 1);
        assert_ne!(after.conversations[0].version, expected.version);
        assert!(!after.items.is_empty());
        assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
    }
}

#[tokio::test]
async fn gitlab_confirmed_response_replays_without_duplicate_effects_and_reassesses_later_edits() {
    let fixture = Fixture::new().await;
    prepare(&fixture);
    let before = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap();
    let expected = response(&before);
    for _ in 0..2 {
        assert_eq!(
            fixture
                .authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &expected,
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential()
                )
                .await
                .unwrap(),
            SourceReviewResolution::Resolved
        );
    }
    assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
    assert_eq!(fixture.writes("/discussions/thread"), 1);
    fixture.state.lock().unwrap().conversation.as_mut().unwrap()["notes"][0]["body"] =
        json!("Substantive later edit.");
    assert_eq!(
        fixture
            .authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &expected,
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential()
            )
            .await
            .unwrap(),
        SourceReviewResolution::Changed
    );
    assert_eq!(
        fixture.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"],
        false
    );
}

#[tokio::test]
async fn gitlab_unresolved_discussion_repairs_and_resolves_exact_verified_revision_before_merge() {
    let fixture = Fixture::new().await;
    prepare(&fixture);
    let feedback = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap();
    assert_eq!(feedback.items.len(), 1);
    assert!(feedback.conversations[0].authorized);
    let response = response(&feedback);
    assert_eq!(
        fixture
            .authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response,
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential()
            )
            .await
            .unwrap(),
        SourceReviewResolution::Resolved
    );
    let after = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap();
    assert!(after.conversations.is_empty());
    assert!(after.items.is_empty());
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap(),
        SourceMergeRequestOutcome::Accepted
    );
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.review.as_ref().unwrap()["state"], "merged");
    assert!(
        state.conversation.as_ref().unwrap()["notes"][1]["body"]
            .as_str()
            .unwrap()
            .contains("\\/close")
    );
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|(method, path, _)| method == "PUT" && path.ends_with("/discussions/thread"))
            .count(),
        1
    );
}

#[tokio::test]
async fn gitlab_edits_and_new_replies_invalidate_the_verified_disposition() {
    for race in [
        "edit_before_response",
        "new_reply",
        "edit_after_reply",
        "edit_after_resolve",
    ] {
        let fixture = Fixture::new().await;
        prepare(&fixture);
        let feedback = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        {
            let mut state = fixture.state.lock().unwrap();
            state.conversation_race = Some(race.into());
            if race == "edit_before_response" {
                state.conversation.as_mut().unwrap()["notes"][0]["body"] = json!("Edited finding.");
            }
            if race == "new_reply" {
                state.conversation.as_mut().unwrap()["notes"]
                    .as_array_mut()
                    .unwrap()
                    .push(note(2, "New substantive reply."));
            }
        }
        assert_eq!(
            fixture
                .authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &response(&feedback),
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential()
                )
                .await
                .unwrap(),
            SourceReviewResolution::Changed,
            "{race}"
        );
        assert!(
            !fixture
                .authority
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap()
                .conversations
                .is_empty(),
            "{race}"
        );
    }
}

#[tokio::test]
async fn gitlab_bot_metadata_never_bypasses_project_membership_for_blocking_discussions() {
    let fixture = Fixture::new().await;
    prepare(&fixture);
    fixture.state.lock().unwrap().conversation.as_mut().unwrap()["notes"][0]["author"]["bot"] =
        json!(true);
    fixture.response(
        "GET",
        "/members/all/31",
        (
            StatusCode::OK,
            json!({"id":31,"state":"active","membership_state":"active","access_level":20}),
        ),
    );
    let feedback = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap();
    assert!(feedback.items.is_empty());
    assert!(!feedback.conversations[0].authorized);
    assert_eq!(
        fixture
            .authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response(&feedback),
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential()
            )
            .await
            .unwrap(),
        SourceReviewResolution::Changed
    );
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|(method, _, _)| method == "GET")
    );
}

#[tokio::test]
async fn gitlab_merge_rejection_race_refreshes_an_authoritative_discussion_blocker() {
    let fixture = Fixture::new().await;
    prepare(&fixture);
    let mut ready = merge_request();
    ready["detailed_merge_status"] = json!("mergeable");
    let mut blocked = ready.clone();
    blocked["detailed_merge_status"] = json!("discussions_not_resolved");
    blocked["blocking_discussions_resolved"] = json!(false);
    fixture.state.lock().unwrap().review_sequence = vec![ready, blocked];
    fixture.response(
        "PUT",
        "/merge",
        (
            StatusCode::METHOD_NOT_ALLOWED,
            json!({"message":"Method Not Allowed"}),
        ),
    );
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap(),
        SourceMergeRequestOutcome::ReviewBlocked
    );
    let feedback = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap();
    assert_eq!(feedback.items.len(), 1);
    assert!(feedback.conversations[0].authorized);
}

#[tokio::test]
async fn gitlab_incomplete_discussion_resolution_status_is_an_api_schema_failure() {
    for status in [None, Some(serde_json::Value::Null)] {
        let fixture = Fixture::new().await;
        prepare(&fixture);
        {
            let mut data = fixture.state.lock().unwrap();
            let note = &mut data.conversation.as_mut().unwrap()["notes"][0];
            if let Some(status) = status {
                note["resolved"] = status;
            } else {
                note.as_object_mut().unwrap().remove("resolved");
            }
        }
        let error = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap_err();
        assert!(matches!(error, SourceDeliveryError::Api(_)), "{error:?}");
        assert_eq!(fixture.writes("/merge"), 0);
        assert_eq!(fixture.writes("/discussions/thread/notes"), 0);
    }
}

#[test]
fn gitlab_note_decoder_requires_status_only_for_resolvable_notes() {
    for status in [
        None,
        Some(Value::Null),
        Some(json!(false)),
        Some(json!(true)),
    ] {
        for resolvable in [false, true] {
            let mut value = note(1, "Review feedback");
            value["resolvable"] = json!(resolvable);
            if let Some(status) = &status {
                value["resolved"] = status.clone();
            } else {
                value.as_object_mut().unwrap().remove("resolved");
            }
            let decoded = serde_json::from_value::<wire::Note>(value);
            let complete = status.as_ref().and_then(Value::as_bool).is_some();
            assert_eq!(decoded.is_ok(), !resolvable || complete);
        }
    }
}

#[tokio::test]
async fn gitlab_mixed_participants_keep_the_maintainer_blocker_despite_authorized_findings() {
    let fixture = Fixture::new().await;
    prepare(&fixture);
    {
        let mut data = fixture.state.lock().unwrap();
        let mut reply = note(2, "Unauthorized request.");
        reply["author"]["id"] = json!(32);
        data.conversation.as_mut().unwrap()["notes"]
            .as_array_mut()
            .unwrap()
            .push(reply);
    }
    fixture.response(
        "GET",
        "/members/all/32",
        (
            StatusCode::OK,
            json!({"id":32,"state":"active","membership_state":"active","access_level":20}),
        ),
    );
    let feedback = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap();
    assert_eq!(feedback.items.len(), 1);
    assert!(!feedback.items[0].body.contains("Unauthorized request"));
    assert!(!feedback.conversations[0].authorized);
    assert_eq!(
        fixture
            .authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response(&feedback),
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint,
                },
                credential(),
            )
            .await
            .unwrap(),
        SourceReviewResolution::Changed
    );
    assert_eq!(fixture.writes("/discussions/thread/notes"), 0);
    assert_eq!(fixture.writes("/discussions/thread"), 0);
}

#[path = "conversation_tests/recovery_tests.rs"]
mod recovery_tests;
