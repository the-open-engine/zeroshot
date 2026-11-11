use super::*;
use super::test_support::*;
use super::super::observation::test_support::{authority, receipt};
use serde_json::json;

fn response(feedback: &SourceReviewFeedback) -> SourceReviewResponse {
    SourceReviewResponse {
        thread_id: feedback.conversations[0].id.clone(),
        version: feedback.conversations[0].version.clone(),
        disposition: crate::source_code_provider::SourceReviewDisposition::Addressed,
        response: "Added the empty-input guard and checked the regression.".into(),
    }
}

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
        .inspect_review_feedback(&receipt(), credential)
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
            .inspect_review_feedback(&receipt(), credential)
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
            .request_merge(&receipt(), credential)
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
        .inspect_review_feedback(&receipt(), credential)
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
            .inspect_review_feedback(&receipt(), credential)
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
            .inspect_review_feedback(&receipt(), credential)
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
        .inspect_review_feedback(&receipt(), credential)
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
async fn restart_without_workspace_snapshots_rechecks_earlier_resolutions_at_both_gates() {
    check_restart_receipts(false).await;
}

#[tokio::test]
async fn interrupted_delivery_rechecks_earlier_resolutions_after_runtime_loss_or_failure() {
    check_restart_receipts(true).await;
}

async fn check_restart_receipts(interrupted: bool) {
    for merge_gate in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let program = fixture(root.path(), &receipt(), "Bot");
        let authority = authority(program.clone(), root.path());
        let credential = SourceDeliveryCredential::token("test-token");
        let before = authority
            .inspect_review_feedback(&receipt(), credential)
            .await
            .unwrap();
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
        assert!(
            authority
                .inspect_review_feedback(&receipt(), credential)
                .await
                .unwrap()
                .conversations
                .is_empty()
        );
        let checkpoint = authority
            .review_conversation_state()
            .await
            .unwrap()
            .unwrap();
        let checkpoint = if interrupted {
            crate::native_v2_runner::test_support::interrupted_delivery_state(
                &root.path().join("lineage"),
                checkpoint,
            )
            .await
        } else {
            crate::native_v2_runner::test_support::restart_delivery_state(
                &root.path().join("lineage"),
                checkpoint,
            )
            .await
        };
        drop(authority);
        let authority = crate::source_code_provider::adapters::core::github::observation::test_support::authority(program.clone(), root.path());
        authority
            .restore_review_conversation_state(checkpoint)
            .await
            .unwrap();
        assert!(
            authority
                .inspect_review_feedback(&receipt(), credential)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        // A later CI repair supplies no review responses. Its head moves, and an
        // earlier resolved finding changes while the worker is repairing CI.
        let mut repaired = receipt();
        repaired.head_revision = "c".repeat(40);
        let path = format!("{}.review", program.display());
        let mut review: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        review["head"]["sha"] = json!(repaired.head_revision);
        std::fs::write(path, review.to_string()).unwrap();
        let mut data = state(&program);
        data["threads"][0]["comments"][0]["body"] = json!("New finding during CI repair.");
        data["threads"][0]["comments"][0]["updated_at"] = json!("two");
        save(&program, &data);
        if merge_gate {
            assert_eq!(
                authority
                    .request_merge(&repaired, credential)
                    .await
                    .unwrap(),
                SourceMergeRequestOutcome::ReviewBlocked
            );
        }
        let after = authority
            .inspect_review_feedback(&repaired, credential)
            .await
            .unwrap();
        assert_eq!(after.conversations.len(), 1);
        assert!(after.conversations[0].authorized);
        assert_ne!(after.items[0].version, before.items[0].version);
        assert!(
            after.items[0]
                .body
                .contains("New finding during CI repair.")
        );
        assert_eq!(state(&program)["threads"][0]["resolved"], false);
        assert_eq!(
            state(&program)["threads"][0]["comments"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            !state(&program)["calls"]
                .to_string()
                .contains("enqueuePullRequest")
        );
    }
}

#[tokio::test]
async fn untrusted_reply_during_failed_confirmation_reopens_without_creating_repair_feedback() {
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "Bot");
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let before = authority
        .inspect_review_feedback(&receipt(), credential)
        .await
        .unwrap();
    let mut data = state(&program);
    data["race"] = json!("unauthorized_reply_after_resolve");
    data["fail_confirmation_reads"] = json!(1);
    save(&program, &data);
    assert!(
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
            .is_err()
    );
    let after = authority
        .inspect_review_feedback(&receipt(), credential)
        .await
        .unwrap();
    assert!(!after.conversations[0].authorized);
    assert_ne!(
        after.conversations[0].version,
        before.conversations[0].version
    );
    assert_eq!(after.items, before.items);
    assert_eq!(state(&program)["threads"][0]["resolved"], false);
    assert_eq!(
        authority
            .inspect_review_feedback(&receipt(), credential)
            .await
            .unwrap(),
        after
    );
}

#[tokio::test]
async fn interrupted_confirmation_reconciles_resolved_revisions_without_duplicate_replies() {
    for changed in [false, true] {
        for failure in [
            "fail_confirmation_reads",
            "lose_resolve_response",
            "lose_reopen_response",
        ] {
            if failure == "lose_reopen_response" && !changed {
                continue;
            }
            let root = tempfile::tempdir().unwrap();
            let program = fixture(root.path(), &receipt(), "Bot");
            let authority = authority(program.clone(), root.path());
            let credential = SourceDeliveryCredential::token("test-token");
            let before = authority
                .inspect_review_feedback(&receipt(), credential)
                .await
                .unwrap();
            let expected = response(&before);
            let mut data = state(&program);
            data[failure] = if failure == "fail_confirmation_reads" {
                json!(1)
            } else {
                json!(true)
            };
            if changed {
                data["race"] = json!("edit_after_resolve");
            }
            save(&program, &data);
            let error = authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &expected,
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint,
                    },
                    credential,
                )
                .await
                .unwrap_err();
            assert!(error.retryable_operation(), "{error}");
            assert_eq!(
                state(&program)["threads"][0]["resolved"],
                failure != "lose_reopen_response"
            );
            // Retry the exact pending attempt, including when no unresolved feedback is visible.
            assert_eq!(
                authority
                    .clone()
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
                if changed {
                    SourceReviewResolution::Changed
                } else {
                    SourceReviewResolution::Resolved
                }
            );
            assert_eq!(state(&program)["threads"][0]["resolved"], !changed);
            assert_eq!(
                state(&program)["threads"][0]["comments"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            let after = authority
                .inspect_review_feedback(&receipt(), credential)
                .await
                .unwrap();
            assert_eq!(after.conversations.is_empty(), !changed);
            if changed {
                assert_ne!(after.conversations[0].version, expected.version);
            }
        }
    }
}

#[tokio::test]
async fn pending_confirmation_blocks_feedback_reads_and_merge_until_changed_thread_is_reopened() {
    for inspection in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let program = fixture(root.path(), &receipt(), "Bot");
        let authority = authority(program.clone(), root.path());
        let credential = SourceDeliveryCredential::token("test-token");
        let before = authority
            .inspect_review_feedback(&receipt(), credential)
            .await
            .unwrap();
        let mut data = state(&program);
        data["race"] = json!("edit_after_resolve");
        data["fail_confirmation_reads"] = json!(2);
        save(&program, &data);
        assert!(
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
                .is_err()
        );
        if inspection {
            assert!(
                authority
                    .inspect_review_feedback(&receipt(), credential)
                    .await
                    .is_err()
            );
            let after = authority
                .inspect_review_feedback(&receipt(), credential)
                .await
                .unwrap();
            assert_eq!(after.conversations.len(), 1);
            assert!(!after.items.is_empty());
        } else {
            assert!(
                authority
                    .request_merge(&receipt(), credential)
                    .await
                    .is_err()
            );
            assert_eq!(
                authority
                    .request_merge(&receipt(), credential)
                    .await
                    .unwrap(),
                SourceMergeRequestOutcome::ReviewBlocked
            );
        }
        assert_eq!(state(&program)["threads"][0]["resolved"], false);
        assert!(
            !state(&program)["calls"]
                .to_string()
                .contains("enqueuePullRequest")
        );
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
async fn pushed_head_positions_preserve_conversation_revision_and_full_comment_identity() {
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "Bot");
    let mut data = state(&program);
    data["threads"][0]["comments"][0]["id"] = json!(8_589_934_592_u64);
    save(&program, &data);
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let before = authority
        .inspect_review_feedback(&receipt(), credential)
        .await
        .unwrap();
    data = state(&program);
    data["threads"][0]["comments"][0]["commit_id"] = json!("c".repeat(40));
    data["threads"][0]["comments"][0]["line"] = json!(17);
    save(&program, &data);
    let after = authority
        .inspect_review_feedback(&receipt(), credential)
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
        .inspect_review_feedback(&receipt(), credential)
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
            .inspect_review_feedback(&receipt(), credential)
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
            .inspect_review_feedback(&receipt(), SourceDeliveryCredential::token("test-token"))
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
            .inspect_review_feedback(&receipt(), credential)
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
        .inspect_review_feedback(&receipt(), credential)
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
        .inspect_review_feedback(&receipt(), credential)
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
        .inspect_review_feedback(&receipt(), credential)
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

#[tokio::test]
async fn checkpoint_recovery_settles_interrupted_confirmation_without_response_replay() {
    for changed in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let program = fixture(root.path(), &receipt(), "Bot");
        let first = authority(program.clone(), root.path());
        let credential = SourceDeliveryCredential::token("test-token");
        let before = first
            .inspect_review_feedback(&receipt(), credential)
            .await
            .unwrap();
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
                        response: &response(&before),
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential
                )
                .await
                .is_err()
        );
        let checkpoint = first.review_conversation_state().await.unwrap().unwrap();
        drop(first);
        let recovered = authority(program.clone(), root.path());
        recovered
            .restore_review_conversation_state(checkpoint)
            .await
            .unwrap();
        let after = recovered
            .inspect_review_feedback(&receipt(), credential)
            .await
            .unwrap();
        assert_eq!(after.conversations.len(), usize::from(changed));
        assert_eq!(after.items.len(), usize::from(changed));
        assert_eq!(state(&program)["threads"][0]["resolved"], !changed);
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
async fn interrupted_delivery_driver_recovers_app_receipts_before_ci_settlement() {
    struct DriverFixture {
        program: std::path::PathBuf,
        root: std::path::PathBuf,
    }
    impl crate::native_v2_delivery::ConversationFixture for DriverFixture {
        fn authority(&self) -> Arc<dyn SourceDeliveryAuthority> {
            Arc::new(authority(self.program.clone(), &self.root))
        }
        fn credential(&self) -> SourceDeliveryCredential<'static> {
            SourceDeliveryCredential::token("test-token")
        }
        fn bind(&self, review: &SourceReviewReceipt) {
            std::fs::write(format!("{}.review", self.program.display()), json!({
                "number":17,"base":{"ref":review.target_branch,"sha":"a".repeat(40),"repo":{"full_name":review.repository}},
                "head":{"ref":review.head_branch,"sha":review.head_revision,"repo":{"full_name":review.repository}}
            }).to_string()).unwrap();
        }
        fn resolved(&self) -> bool {
            state(&self.program)["threads"][0]["resolved"] == true
        }
        fn edit(&self) {
            let mut data = state(&self.program);
            data["threads"][0]["comments"][0]["body"] =
                json!("Substantive finding after controller loss.");
            data["threads"][0]["comments"][0]["updated_at"] = json!("edited");
            save(&self.program, &data);
        }
    }
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "Bot");
    crate::native_v2_delivery::assert_interrupted_conversation_driver(Arc::new(DriverFixture {
        program,
        root: root.path().into(),
    }))
    .await;
}

#[tokio::test]
async fn github_checkpoint_failure_blocks_resolution_and_retry_reuses_exact_reply() {
    let root = tempfile::tempdir().unwrap();
    let program = fixture(root.path(), &receipt(), "Bot");
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let feedback = authority
        .inspect_review_feedback(&receipt(), credential)
        .await
        .unwrap();
    let response = response(&feedback);
    assert!(
        authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response,
                    checkpoint: &crate::source_code_provider::RefuseReviewCheckpoint,
                },
                credential
            )
            .await
            .is_err()
    );
    assert!(
        authority
            .inspect_review_feedback(&receipt(), credential)
            .await
            .is_err()
    );
    assert!(
        authority
            .request_merge(&receipt(), credential)
            .await
            .is_err()
    );
    assert_eq!(state(&program)["threads"][0]["resolved"], false);
    assert_eq!(
        state(&program)["threads"][0]["comments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response,
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint,
                },
                credential
            )
            .await
            .unwrap(),
        SourceReviewResolution::Resolved
    );
    assert_eq!(
        state(&program)["threads"][0]["comments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
