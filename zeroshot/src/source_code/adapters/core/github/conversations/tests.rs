use crate::source_code_provider::TestReviewCheckpoint;
use super::*;
use super::test_support::*;
use super::super::observation::test_support::{authority, receipt};
use serde_json::json;

#[tokio::test]
async fn large_verified_responses_retain_compact_receipts_across_repair_rounds() {
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "Bot");
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let mut data = state(&program);
    let template = data["threads"][0].clone();
    data["threads"] = json!(
        (0..10)
            .map(|index| {
                let mut thread = template.clone();
                thread["id"] = json!(format!("THREAD_{index}"));
                thread["comments"][0]["id"] = json!(41 + index);
                thread
            })
            .collect::<Vec<_>>()
    );
    save(&program, &data);
    let feedback = authority
        .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
        .await
        .unwrap();
    let responses: Vec<_> = feedback
        .conversations
        .iter()
        .map(|thread| SourceReviewResponse {
            thread_id: thread.id.clone(),
            version: thread.version.clone(),
            disposition: crate::source_code_provider::SourceReviewDisposition::Addressed,
            response: "x".repeat(60_000),
        })
        .collect();
    assert!(serde_json::to_vec(&responses).unwrap().len() < 1024 * 1024);
    for response in &responses {
        assert_eq!(
            authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response,
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential
                )
                .await
                .unwrap(),
            SourceReviewResolution::Resolved
        );
    }
    let checkpoint = authority
        .review_conversation_state()
        .await
        .unwrap()
        .unwrap();
    assert!(serde_json::to_vec(&checkpoint).unwrap().len() < 10_000);
    review_responses::assert_fits_ledger(checkpoint.clone());
    let recovered =
        crate::source_code_provider::adapters::core::github::observation::test_support::authority(
            program.clone(),
            root.path(),
        );
    recovered
        .restore_review_conversation_state(checkpoint)
        .await
        .unwrap();
    // A subsequent repair has no responses. All earlier owned resolutions still revalidate.
    assert!(
        recovered
            .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let mut data = state(&program);
    data["threads"][0]["comments"][0]["body"] = json!("Later substantive edit.");
    save(&program, &data);
    assert_eq!(
        recovered
            .request_merge(&receipt(), credential, &TestReviewCheckpoint)
            .await
            .unwrap(),
        SourceMergeRequestOutcome::ReviewBlocked
    );
}

#[tokio::test]
async fn accumulated_receipt_limit_rejects_before_posting_or_resolving() {
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "Bot");
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let checkpoint = review_responses::fill_receipt_storage(
        authority
            .review_conversation_state()
            .await
            .unwrap()
            .unwrap(),
        &receipt(),
    );
    authority
        .restore_review_conversation_state(checkpoint)
        .await
        .unwrap();
    let retained = authority
        .review_conversation_state()
        .await
        .unwrap()
        .unwrap();
    review_responses::assert_fits_ledger(retained.clone());
    let feedback = authority
        .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
        .await
        .unwrap();
    assert!(
        authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response(&feedback),
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential
            )
            .await
            .is_err()
    );
    assert_eq!(
        authority
            .review_conversation_state()
            .await
            .unwrap()
            .unwrap(),
        retained
    );
    let data = state(&program);
    assert_eq!(data["threads"][0]["comments"].as_array().unwrap().len(), 1);
    assert_eq!(data["threads"][0]["resolved"], false);
    assert!(!data["calls"].as_array().unwrap().iter().any(|args| {
        args.as_array()
            .unwrap()
            .iter()
            .any(|arg| arg.as_str().unwrap().contains("mutation("))
    }));
}

#[tokio::test]
async fn replayed_response_after_authority_reconstruction_reopens_unproven_resolved_revision() {
    for changed in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let program = fixture(root.path(), &receipt(), "Bot");
        let first = authority(program.clone(), root.path());
        let credential = SourceDeliveryCredential::token("test-token");
        let before = first
            .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
            .await
            .unwrap();
        let expected = response(&before);
        let mut data = state(&program);
        data["fail_confirmation_reads"] = json!(1);
        if changed {
            data["race"] = json!("edit_after_resolve");
        }
        save(&program, &data);
        assert!(
            first
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &expected,
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential
                )
                .await
                .is_err()
        );
        assert_eq!(state(&program)["threads"][0]["resolved"], true);
        let recovered = authority(program.clone(), root.path());
        assert_eq!(
            recovered
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &expected,
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential
                )
                .await
                .unwrap(),
            SourceReviewResolution::Changed
        );
        let after = recovered
            .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
            .await
            .unwrap();
        assert_eq!(after.conversations.len(), 1);
        assert_ne!(after.conversations[0].version, expected.version);
        assert!(!after.items.is_empty());
        assert_eq!(state(&program)["threads"][0]["resolved"], false);
        assert_eq!(
            state(&program)["threads"][0]["comments"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}

#[tokio::test]
async fn confirmed_response_replays_without_duplicate_effects_and_reassesses_later_edits() {
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "Bot");
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let before = authority
        .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
        .await
        .unwrap();
    let expected = response(&before);
    for _ in 0..2 {
        assert_eq!(
            authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &expected,
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential
                )
                .await
                .unwrap(),
            SourceReviewResolution::Resolved
        );
    }
    let mut data = state(&program);
    assert_eq!(data["threads"][0]["comments"].as_array().unwrap().len(), 2);
    assert_eq!(
        data["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|call| call
                .to_string()
                .contains("query=mutation($thread:ID!){resolveReviewThread"))
            .count(),
        1
    );
    data["threads"][0]["comments"][0]["body"] = json!("Substantive later edit.");
    save(&program, &data);
    assert_eq!(
        authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &expected,
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential
            )
            .await
            .unwrap(),
        SourceReviewResolution::Changed
    );
    assert_eq!(state(&program)["threads"][0]["resolved"], false);
}

#[tokio::test]
async fn pushed_head_positions_preserve_conversation_revision_and_full_comment_identity() {
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "Bot");
    let mut data = state(&program);
    data["threads"][0]["comments"][0]["id"] = json!(8_589_934_592_u64);
    save(&program, &data);
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let before = authority
        .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
        .await
        .unwrap();
    data = state(&program);
    data["threads"][0]["comments"][0]["commit_id"] = json!("c".repeat(40));
    data["threads"][0]["comments"][0]["line"] = json!(17);
    save(&program, &data);
    let after = authority
        .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
        .await
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response(&before),
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential
            )
            .await
            .unwrap(),
        SourceReviewResolution::Resolved
    );
}

#[tokio::test]
async fn app_feedback_conversation_needs_no_collaborator_or_installation_lookup() {
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "Bot");
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let feedback = authority
        .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
        .await
        .unwrap();
    assert_eq!(feedback.items.len(), 1);
    assert!(feedback.conversations[0].authorized);
    let expected = response(&feedback);
    assert_eq!(
        authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &expected,
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential
            )
            .await
            .unwrap(),
        SourceReviewResolution::Resolved
    );
    assert!(
        authority
            .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
            .await
            .unwrap()
            .conversations
            .is_empty()
    );
    let state = state(&program);
    assert!(state["threads"][0]["resolved"].as_bool().unwrap());
    assert_eq!(state["threads"][0]["comments"].as_array().unwrap().len(), 2);
    assert!(
        state["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| !call.to_string().contains("collaborators")
                && !call.to_string().contains("installation"))
    );
}

#[tokio::test]
async fn human_app_actions_require_current_write_and_bot_suffixes_have_no_authority() {
    for permission in ["read", "write", "admin"] {
        let root = tempfile::tempdir().unwrap();
        let program = fixture(root.path(), &receipt(), "User");
        let mut data = state(&program);
        data["permission"] = json!(permission);
        data["threads"][0]["comments"][0]["user"]["login"] = json!("looks-like-an-app[bot]");
        data["threads"][0]["comments"][0]["performed_via_github_app"] = json!({"id":999});
        save(&program, &data);
        let authority = authority(program.clone(), root.path());
        let feedback = authority
            .inspect_review_feedback(
                &receipt(),
                SourceDeliveryCredential::token("test-token"),
                &TestReviewCheckpoint,
            )
            .await
            .unwrap();
        assert_eq!(feedback.items.is_empty(), permission == "read");
        assert_eq!(feedback.conversations[0].authorized, permission != "read");
        assert!(
            state(&program)["calls"]
                .to_string()
                .contains("collaborators")
        );
    }
}

#[tokio::test]
async fn stale_edits_replies_and_resolution_races_require_reassessment() {
    for race in [
        "edit_before_response",
        "edit_after_reply",
        "reply_after_reply",
        "edit_after_resolve",
    ] {
        let root = tempfile::tempdir().unwrap();
        let program = fixture(root.path(), &receipt(), "Bot");
        let authority = authority(program.clone(), root.path());
        let credential = SourceDeliveryCredential::token("test-token");
        let before = authority
            .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
            .await
            .unwrap();
        let mut data = state(&program);
        data["race"] = json!(race);
        if race == "edit_before_response" {
            data["threads"][0]["comments"][0]["body"] = json!("Edited finding");
        }
        save(&program, &data);
        assert_eq!(
            authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &response(&before),
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential
                )
                .await
                .unwrap(),
            SourceReviewResolution::Changed,
            "{race}"
        );
        let data = state(&program);
        assert_eq!(data["threads"][0]["resolved"], false, "{race}");
        if race == "edit_before_response" {
            assert_eq!(data["threads"][0]["comments"].as_array().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn a_response_never_resolves_a_different_thread_or_a_readers_blocker() {
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "User");
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let feedback = authority
        .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
        .await
        .unwrap();
    let mut response = response(&feedback);
    assert_eq!(
        authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response,
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential
            )
            .await
            .unwrap(),
        SourceReviewResolution::Changed
    );
    response.thread_id = "OTHER".into();
    assert_eq!(
        authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response,
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential
            )
            .await
            .unwrap(),
        SourceReviewResolution::Changed
    );
    assert!(
        state(&program)["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| !call.to_string().contains("mutation"))
    );
}

#[tokio::test]
async fn unauthorized_replies_do_not_hide_authorized_findings_or_create_new_repair_versions() {
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "Bot");
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let before = authority
        .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
        .await
        .unwrap();
    let mut data = state(&program);
    let mut reply = data["threads"][0]["comments"][0].clone();
    reply["id"] = json!(42);
    reply["in_reply_to_id"] = json!(41);
    reply["body"] = json!("Unauthorized request.");
    reply["user"] = json!({"id":9,"login":"reader","type":"User"});
    data["threads"][0]["comments"]
        .as_array_mut()
        .unwrap()
        .push(reply);
    save(&program, &data);
    let after = authority
        .inspect_review_feedback(&receipt(), credential, &TestReviewCheckpoint)
        .await
        .unwrap();
    assert_eq!(after.items.len(), 1);
    assert_eq!(before.items[0].version, after.items[0].version);
    assert_ne!(
        before.conversations[0].version,
        after.conversations[0].version
    );
    assert!(!after.conversations[0].authorized);
    assert!(!after.items[0].body.contains("Unauthorized request"));
}
