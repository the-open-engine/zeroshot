use super::*;

#[tokio::test]
async fn gitlab_restart_without_workspace_snapshots_rechecks_earlier_resolutions_at_both_gates() {
    check_restart_receipts(false).await;
}

#[tokio::test]
async fn gitlab_interrupted_delivery_rechecks_resolutions_after_runtime_loss_or_failure() {
    check_restart_receipts(true).await;
}

async fn check_restart_receipts(interrupted: bool) {
    for merge_gate in [false, true] {
        let fixture = Fixture::new().await;
        prepare(&fixture);
        let before = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        assert_eq!(
            fixture
                .authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &response(&before),
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential()
                )
                .await
                .unwrap(),
            SourceReviewResolution::Resolved
        );
        assert!(
            fixture
                .authority
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap()
                .conversations
                .is_empty()
        );
        let checkpoint = fixture
            .authority
            .review_conversation_state()
            .await
            .unwrap()
            .unwrap();
        let lineage = tempfile::tempdir().unwrap();
        let checkpoint = if interrupted {
            crate::native_v2_runner::test_support::interrupted_delivery_state(
                &lineage.path().join("lineage"),
                checkpoint,
            )
            .await
        } else {
            crate::native_v2_runner::test_support::restart_delivery_state(
                &lineage.path().join("lineage"),
                checkpoint,
            )
            .await
        };
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
        // The next CI repair has no review responses to replay. Reconcile the
        // retained verified revision independently of that worker output.
        let mut repaired = receipt();
        repaired.head_revision = "c".repeat(40);
        {
            let mut data = fixture.state.lock().unwrap();
            let review = data.review.as_mut().unwrap();
            review["sha"] = json!(repaired.head_revision);
            review["head_pipeline"]["sha"] = json!(repaired.head_revision);
            data.conversation.as_mut().unwrap()["notes"][0]["body"] =
                json!("New finding during CI repair.");
        }
        if merge_gate {
            assert_eq!(
                recovered
                    .request_merge(&repaired, credential(), &TestReviewCheckpoint)
                    .await
                    .unwrap(),
                SourceMergeRequestOutcome::ReviewBlocked
            );
        }
        let after = recovered
            .inspect_review_feedback(&repaired, credential(), &TestReviewCheckpoint)
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
        assert_eq!(fixture.writes("/merge"), 0);
        assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
        assert_eq!(
            fixture.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"],
            false
        );
    }
}

#[tokio::test]
async fn gitlab_untrusted_reply_during_confirmation_does_not_create_repair_feedback() {
    let fixture = Fixture::new().await;
    prepare(&fixture);
    let before = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap();
    fixture.state.lock().unwrap().confirmation_failures = 1;
    assert!(
        fixture
            .authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response(&before),
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                },
                credential()
            )
            .await
            .is_err()
    );
    {
        let mut data = fixture.state.lock().unwrap();
        let mut reply = note(502, "Unauthorized request.");
        reply["author"]["id"] = json!(99);
        reply["resolvable"] = json!(true);
        reply["resolved"] = json!(true);
        data.conversation.as_mut().unwrap()["notes"]
            .as_array_mut()
            .unwrap()
            .push(reply);
    }
    fixture.response(
        "GET",
        "/members/all/99",
        (
            StatusCode::OK,
            json!({"id":99,"state":"active","membership_state":"active","access_level":20}),
        ),
    );
    let after = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap();
    assert!(!after.conversations[0].authorized);
    assert_ne!(
        after.conversations[0].version,
        before.conversations[0].version
    );
    assert_eq!(after.items, before.items);
    assert_eq!(
        fixture.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"],
        false
    );
    assert_eq!(
        fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap(),
        after
    );
}

#[tokio::test]
async fn gitlab_interrupted_confirmation_retains_exact_pending_resolution_until_verified() {
    for changed in [false, true] {
        for failure in ["confirmation", "resolve", "reopen"] {
            if failure == "reopen" && !changed {
                continue;
            }
            let fixture = Fixture::new().await;
            prepare(&fixture);
            let feedback = fixture
                .authority
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap();
            let expected = response(&feedback);
            {
                let mut data = fixture.state.lock().unwrap();
                match failure {
                    "confirmation" => data.confirmation_failures = 1,
                    "resolve" => data.lose_resolve_response = true,
                    "reopen" => data.lose_reopen_response = true,
                    _ => unreachable!(),
                }
                if changed {
                    data.conversation_race = Some("edit_after_resolve".into());
                }
            }
            let error = fixture
                .authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &expected,
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint,
                    },
                    credential(),
                )
                .await
                .unwrap_err();
            assert!(error.retryable_operation(), "{error}");
            assert_eq!(
                fixture.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"],
                failure != "reopen"
            );
            assert_eq!(
                fixture
                    .authority
                    .clone()
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
                if changed {
                    SourceReviewResolution::Changed
                } else {
                    SourceReviewResolution::Resolved
                }
            );
            let after = fixture
                .authority
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap();
            assert_eq!(after.conversations.is_empty(), !changed);
            assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
            if changed {
                assert_ne!(after.conversations[0].version, expected.version);
            } else {
                assert_eq!(
                    fixture
                        .authority
                        .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
                        .await
                        .unwrap(),
                    SourceMergeRequestOutcome::Accepted
                );
            }
        }
    }
}

#[tokio::test]
async fn gitlab_pending_confirmation_gates_feedback_and_merge_until_reopening_is_confirmed() {
    for inspection in [true, false] {
        let fixture = Fixture::new().await;
        prepare(&fixture);
        let before = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        {
            let mut data = fixture.state.lock().unwrap();
            data.conversation_race = Some("edit_after_resolve".into());
            data.confirmation_failures = 2;
        }
        assert!(
            fixture
                .authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &response(&before),
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential()
                )
                .await
                .is_err()
        );
        if inspection {
            assert!(
                fixture
                    .authority
                    .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                    .await
                    .is_err()
            );
            let after = fixture
                .authority
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap();
            assert_eq!(after.conversations.len(), 1);
            assert!(!after.items.is_empty());
        } else {
            assert!(
                fixture
                    .authority
                    .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
                    .await
                    .is_err()
            );
            assert_eq!(
                fixture
                    .authority
                    .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
                    .await
                    .unwrap(),
                SourceMergeRequestOutcome::ReviewBlocked
            );
        }
        assert_eq!(
            fixture.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"],
            false
        );
        assert_eq!(fixture.writes("/merge"), 0);
        assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
    }
}

#[tokio::test]
async fn gitlab_checkpoint_recovery_settles_interrupted_confirmation_without_response_replay() {
    for changed in [false, true] {
        let fixture = Fixture::new().await;
        prepare(&fixture);
        let before = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
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
                        response: &response(&before),
                        checkpoint: &crate::source_code_provider::TestReviewCheckpoint
                    },
                    credential()
                )
                .await
                .is_err()
        );
        let checkpoint = fixture
            .authority
            .review_conversation_state()
            .await
            .unwrap()
            .unwrap();
        let mut recovered =
            GitLabDeliveryAuthority::new(fixture.authority.base_url.clone(), None).unwrap();
        recovered.api = fixture.authority.api.clone();
        recovered
            .restore_review_conversation_state(checkpoint)
            .await
            .unwrap();
        let after = recovered
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        assert_eq!(after.conversations.len(), usize::from(changed));
        assert_eq!(after.items.len(), usize::from(changed));
        assert_eq!(
            fixture.state.lock().unwrap().conversation.as_ref().unwrap()["notes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            fixture.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"],
            !changed
        );
        assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn gitlab_interrupted_delivery_driver_recovers_receipts_before_ci_settlement() {
    struct DriverFixture(Fixture);
    impl crate::native_v2_delivery::ConversationFixture for DriverFixture {
        fn authority(&self) -> Arc<dyn SourceDeliveryAuthority> {
            let mut authority =
                GitLabDeliveryAuthority::new(self.0.authority.base_url.clone(), None).unwrap();
            authority.api = self.0.authority.api.clone();
            Arc::new(authority)
        }
        fn credential(&self) -> SourceDeliveryCredential<'static> {
            credential()
        }
        fn bind(&self, review: &SourceReviewReceipt) {
            let mut data = self.0.state.lock().unwrap();
            data.project["path_with_namespace"] = json!(review.repository);
            data.project["web_url"] = json!(format!("{ORIGIN}/{}", review.repository));
            data.responses.insert(
                ("GET".into(), "/members/all/31".into()),
                (
                    StatusCode::OK,
                    json!({"id":31,"state":"active","membership_state":"active","access_level":30}),
                ),
            );
            let current = data.review.as_mut().unwrap();
            current["source_branch"] = json!(review.head_branch);
            current["target_branch"] = json!(review.target_branch);
            current["sha"] = json!(review.head_revision);
            current["head_pipeline"]["sha"] = json!(review.head_revision);
        }
        fn resolved(&self) -> bool {
            self.0.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"]
                == true
        }
        fn edit(&self) {
            self.0.state.lock().unwrap().conversation.as_mut().unwrap()["notes"][0]["body"] =
                json!("Substantive finding after controller loss.");
        }
    }
    let fixture = Fixture::new().await;
    prepare(&fixture);
    crate::native_v2_delivery::assert_interrupted_conversation_driver(Arc::new(DriverFixture(
        fixture,
    )))
    .await;
}

#[tokio::test]
async fn gitlab_checkpoint_failure_blocks_resolution_and_retry_reuses_exact_reply() {
    let fixture = Fixture::new().await;
    prepare(&fixture);
    let feedback = fixture
        .authority
        .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
        .await
        .unwrap();
    let response = response(&feedback);
    let checkpoint = crate::source_code_provider::RecordingReviewCheckpoint {
        fail_at: Some(2),
        ..Default::default()
    };
    assert!(
        fixture
            .authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response,
                    checkpoint: &checkpoint,
                },
                credential()
            )
            .await
            .is_err()
    );
    assert!(
        fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
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
    assert_eq!(fixture.writes("/discussions/thread"), 0);
    assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
    assert_eq!(
        fixture
            .authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response,
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint,
                },
                credential()
            )
            .await
            .unwrap(),
        SourceReviewResolution::Resolved
    );
    assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
}

#[tokio::test]
async fn gitlab_lost_reply_acknowledgement_reconciles_durable_intent_after_recovery() {
    for recover in [false, true] {
        for changed in [false, true] {
            let fixture = Fixture::new().await;
            prepare(&fixture);
            let before = fixture
                .authority
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap();
            let response = response(&before);
            let checkpoint = crate::source_code_provider::RecordingReviewCheckpoint::default();
            {
                let mut data = fixture.state.lock().unwrap();
                data.lose_reply_response = true;
                if changed {
                    data.conversation_race = Some("edit_after_reply".into());
                }
            }
            assert!(
                fixture
                    .authority
                    .resolve_review_conversation(
                        crate::source_code_provider::SourceReviewResolutionRequest {
                            review: &receipt(),
                            response: &response,
                            checkpoint: &checkpoint,
                        },
                        credential(),
                    )
                    .await
                    .is_err()
            );
            let durable = checkpoint.states.lock().unwrap()[0].clone();
            assert_eq!(durable["gitlab"][0][1]["reply"], Value::Null);
            assert!(durable["replies"].as_array().unwrap().is_empty());
            let mut recovered =
                GitLabDeliveryAuthority::new(fixture.authority.base_url.clone(), None).unwrap();
            recovered.api = fixture.authority.api.clone();
            let current = if recover {
                recovered
                    .restore_review_conversation_state(durable)
                    .await
                    .unwrap();
                &recovered
            } else {
                &fixture.authority
            };
            if !recover {
                assert_eq!(
                    current
                        .resolve_review_conversation(
                            crate::source_code_provider::SourceReviewResolutionRequest {
                                review: &receipt(),
                                response: &response,
                                checkpoint: &crate::source_code_provider::TestReviewCheckpoint,
                            },
                            credential(),
                        )
                        .await
                        .unwrap(),
                    if changed {
                        SourceReviewResolution::Changed
                    } else {
                        SourceReviewResolution::Resolved
                    }
                );
            }
            let after = current
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap();
            assert_eq!(after.items.len(), usize::from(changed));
            assert_eq!(after.conversations.len(), usize::from(changed));
            if changed {
                assert!(!after.items[0].body.contains("Guarded empty input"));
            }
            assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
            assert_eq!(
                fixture.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"],
                !changed
            );
        }
    }
}

#[tokio::test]
async fn gitlab_uncertain_reply_requires_exact_actor_body_and_unique_identity() {
    for defect in ["actor", "body", "duplicate", "absent"] {
        let fixture = Fixture::new().await;
        prepare(&fixture);
        let before = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        let checkpoint = crate::source_code_provider::RecordingReviewCheckpoint::default();
        {
            let mut data = fixture.state.lock().unwrap();
            data.lose_reply_response = true;
            if defect == "absent" {
                data.conversation_race = Some("reject_reply".into());
            }
        }
        assert!(
            fixture
                .authority
                .resolve_review_conversation(
                    crate::source_code_provider::SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &response(&before),
                        checkpoint: &checkpoint,
                    },
                    credential(),
                )
                .await
                .is_err()
        );
        {
            let mut data = fixture.state.lock().unwrap();
            let notes = data.conversation.as_mut().unwrap()["notes"]
                .as_array_mut()
                .unwrap();
            match defect {
                "actor" => notes[1]["author"]["id"] = json!(32),
                "body" => notes[1]["body"] = json!("Edited response."),
                "duplicate" => {
                    let mut reply = notes[1].clone();
                    reply["id"] = json!(502);
                    notes.push(reply);
                }
                _ => {}
            }
        }
        let mut recovered =
            GitLabDeliveryAuthority::new(fixture.authority.base_url.clone(), None).unwrap();
        recovered.api = fixture.authority.api.clone();
        let durable = checkpoint.states.lock().unwrap()[0].clone();
        recovered
            .restore_review_conversation_state(durable)
            .await
            .unwrap();
        assert!(
            recovered
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .is_err(),
            "{defect}"
        );
        assert!(
            recovered
                .request_merge(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .is_err(),
            "{defect}"
        );
        assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
        assert_eq!(fixture.writes("/discussions/thread"), 0);
    }
}

#[tokio::test]
async fn gitlab_reply_intent_must_be_acknowledged_before_posting() {
    let fixture = Fixture::new().await;
    prepare(&fixture);
    let before = fixture
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
                    response: &response(&before),
                    checkpoint: &crate::source_code_provider::RefuseReviewCheckpoint,
                },
                credential(),
            )
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/discussions/thread/notes"), 0);
    assert_eq!(fixture.writes("/discussions/thread"), 0);
}

#[tokio::test]
async fn gitlab_authentication_rejection_retires_intent_before_refresh_or_recovery() {
    for mode in ["retry", "recover", "retirement_failure"] {
        let fixture = Fixture::new().await;
        prepare(&fixture);
        let before = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        fixture
            .authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response(&before),
                    checkpoint: &crate::source_code_provider::TestReviewCheckpoint,
                },
                credential(),
            )
            .await
            .unwrap();
        let previous = fixture
            .authority
            .review_conversation_state()
            .await
            .unwrap()
            .unwrap();
        {
            let mut data = fixture.state.lock().unwrap();
            data.conversation.as_mut().unwrap()["notes"][0]["body"] =
                json!("Another substantive finding.");
            data.conversation_race = Some("require_fresh_reply".into());
        }
        let before = fixture
            .authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        let response = response(&before);
        let checkpoint = crate::source_code_provider::RecordingReviewCheckpoint {
            fail_at: (mode == "retirement_failure").then_some(2),
            ..Default::default()
        };
        let error = fixture
            .authority
            .resolve_review_conversation(
                crate::source_code_provider::SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response,
                    checkpoint: &checkpoint,
                },
                credential(),
            )
            .await
            .unwrap_err();
        let states = checkpoint.states.lock().unwrap().clone();
        assert_eq!(states.len(), 2, "{mode}");
        assert_eq!(states[0]["gitlab"][0][1]["reply"], Value::Null);
        assert!(states[1]["gitlab"].as_array().unwrap().is_empty());
        assert_eq!(states[1]["replies"], previous["replies"]);
        let mut recovered =
            GitLabDeliveryAuthority::new(fixture.authority.base_url.clone(), None).unwrap();
        recovered.api = fixture.authority.api.clone();
        if mode == "retirement_failure" {
            assert_eq!(
                fixture
                    .authority
                    .review_conversation_state()
                    .await
                    .unwrap()
                    .unwrap(),
                states[0]
            );
            assert!(
                fixture
                    .authority
                    .resolve_review_conversation(
                        crate::source_code_provider::SourceReviewResolutionRequest {
                            review: &receipt(),
                            response: &response,
                            checkpoint: &crate::source_code_provider::RefuseReviewCheckpoint,
                        },
                        credential(),
                    )
                    .await
                    .is_err()
            );
            assert!(!error.authentication_failed());
            assert_eq!(error.api_status(), None);
            recovered
                .restore_review_conversation_state(states[0].clone())
                .await
                .unwrap();
            assert!(
                recovered
                    .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                    .await
                    .is_err()
            );
        } else {
            assert!(error.authentication_failed());
            assert_eq!(error.api_status(), Some(401));
            assert!(error.to_string().contains("Bad credentials"));
            let current = if mode == "recover" {
                recovered
                    .restore_review_conversation_state(states[1].clone())
                    .await
                    .unwrap();
                &recovered
            } else {
                &fixture.authority
            };
            assert_eq!(
                current
                    .resolve_review_conversation(
                        crate::source_code_provider::SourceReviewResolutionRequest {
                            review: &receipt(),
                            response: &response,
                            checkpoint: &crate::source_code_provider::TestReviewCheckpoint,
                        },
                        SourceDeliveryCredential::token("refreshed-private-token"),
                    )
                    .await
                    .unwrap(),
                SourceReviewResolution::Resolved
            );
            assert!(
                current
                    .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                    .await
                    .unwrap()
                    .items
                    .is_empty()
            );
        }
        let finished = mode != "retirement_failure";
        assert_eq!(
            fixture.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"],
            finished
        );
        assert_eq!(
            fixture.state.lock().unwrap().conversation.as_ref().unwrap()["notes"]
                .as_array()
                .unwrap()
                .len(),
            2 + usize::from(finished)
        );
        assert_eq!(
            fixture.writes("/discussions/thread/notes"),
            2 + usize::from(finished)
        );
    }
}

#[tokio::test]
async fn gitlab_reconciliation_acknowledges_progress_at_feedback_and_merge_gates() {
    use crate::source_code_provider::RecordingReviewCheckpoint;

    for merge_gate in [false, true] {
        for mode in [
            "settled",
            "changed",
            "confirmation_error",
            "checkpoint_error",
        ] {
            let fixture = Fixture::new().await;
            prepare(&fixture);
            let before = fixture
                .authority
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap();
            let initial = RecordingReviewCheckpoint::default();
            fixture.state.lock().unwrap().lose_reply_response = true;
            assert!(
                fixture
                    .authority
                    .resolve_review_conversation(
                        crate::source_code_provider::SourceReviewResolutionRequest {
                            review: &receipt(),
                            response: &response(&before),
                            checkpoint: &initial,
                        },
                        credential(),
                    )
                    .await
                    .is_err()
            );
            let intent = initial.states.lock().unwrap()[0].clone();
            let mut recovered =
                GitLabDeliveryAuthority::new(fixture.authority.base_url.clone(), None).unwrap();
            recovered.api = fixture.authority.api.clone();
            recovered
                .restore_review_conversation_state(intent.clone())
                .await
                .unwrap();
            {
                let mut data = fixture.state.lock().unwrap();
                data.review.as_mut().unwrap()["head_pipeline"]["status"] = json!("running");
                if mode == "changed" {
                    data.conversation.as_mut().unwrap()["notes"][0]["body"] =
                        json!("Changed before settlement.");
                }
                if mode == "confirmation_error" {
                    data.confirmation_failures = 1;
                }
            }
            let checkpoint = RecordingReviewCheckpoint {
                fail_at: (mode == "checkpoint_error").then_some(1),
                ..Default::default()
            };
            let result = if merge_gate {
                recovered
                    .request_merge(&receipt(), credential(), &checkpoint)
                    .await
                    .map(|outcome| {
                        assert_eq!(
                            outcome,
                            if mode == "changed" {
                                SourceMergeRequestOutcome::ReviewBlocked
                            } else {
                                SourceMergeRequestOutcome::Pending
                            }
                        );
                    })
            } else {
                recovered
                    .inspect_review_feedback(&receipt(), credential(), &checkpoint)
                    .await
                    .map(|feedback| {
                        assert_eq!(feedback.items.len(), usize::from(mode == "changed"));
                    })
            };
            match mode {
                "confirmation_error" => assert_eq!(result.unwrap_err().api_status(), Some(503)),
                "checkpoint_error" => assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("test receipt persistence failed")
                ),
                _ => result.unwrap(),
            }
            let states = checkpoint.states.lock().unwrap().clone();
            assert_eq!(states.len(), 1, "{merge_gate} {mode}");
            assert_eq!(states[0]["replies"].as_array().unwrap().len(), 1);
            if mode == "changed" {
                assert!(states[0]["gitlab"].as_array().unwrap().is_empty());
            } else {
                assert!(states[0]["gitlab"][0][1]["reply"].is_u64());
                assert_eq!(
                    states[0]["gitlab"][0][1]["confirmed"],
                    mode != "confirmation_error"
                );
            }
            if mode == "checkpoint_error" {
                assert_eq!(
                    recovered
                        .review_conversation_state()
                        .await
                        .unwrap()
                        .unwrap(),
                    intent
                );
                assert!(
                    recovered
                        .inspect_review_feedback(
                            &receipt(),
                            credential(),
                            &crate::source_code_provider::RefuseReviewCheckpoint
                        )
                        .await
                        .is_err()
                );
                assert!(
                    recovered
                        .request_merge(
                            &receipt(),
                            credential(),
                            &crate::source_code_provider::RefuseReviewCheckpoint
                        )
                        .await
                        .is_err()
                );
            }
            let mut successor =
                GitLabDeliveryAuthority::new(fixture.authority.base_url.clone(), None).unwrap();
            successor.api = fixture.authority.api.clone();
            successor
                .restore_review_conversation_state(if mode == "checkpoint_error" {
                    intent
                } else {
                    states[0].clone()
                })
                .await
                .unwrap();
            fixture.state.lock().unwrap().conversation.as_mut().unwrap()["notes"][0]["body"] =
                json!("Substantive edit after recovery.");
            let feedback = successor
                .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
                .await
                .unwrap();
            assert_eq!(feedback.items.len(), 1);
            assert_eq!(feedback.conversations.len(), 1);
            assert_eq!(
                fixture.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"],
                false
            );
            assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
            assert_eq!(fixture.writes("/merge"), 0);
        }
    }
}

#[tokio::test]
async fn gitlab_failed_last_receipt_removal_preserves_authority_until_acknowledged() {
    use crate::source_code_provider::{
        RefuseReviewCheckpoint, RecordingReviewCheckpoint, SourceReviewResolutionRequest,
    };

    for gate in ["feedback", "merge", "response"] {
        let fixture = Fixture::new().await;
        prepare(&fixture);
        let authority = &fixture.authority;
        let before = authority
            .inspect_review_feedback(&receipt(), credential(), &TestReviewCheckpoint)
            .await
            .unwrap();
        let response = response(&before);
        let checkpoint = RecordingReviewCheckpoint::default();
        authority
            .resolve_review_conversation(
                SourceReviewResolutionRequest {
                    review: &receipt(),
                    response: &response,
                    checkpoint: &checkpoint,
                },
                credential(),
            )
            .await
            .unwrap();
        let acknowledged = checkpoint.states.lock().unwrap().last().unwrap().clone();
        assert_eq!(acknowledged["gitlab"].as_array().unwrap().len(), 1);
        fixture.state.lock().unwrap().conversation.as_mut().unwrap()["notes"][0]["body"] =
            json!("Changed after the verified resolution.");

        let refused = RecordingReviewCheckpoint {
            fail_at: Some(if gate == "response" { 2 } else { 1 }),
            ..Default::default()
        };
        let result = match gate {
            "feedback" => authority
                .inspect_review_feedback(&receipt(), credential(), &refused)
                .await
                .map(|_| ()),
            "merge" => authority
                .request_merge(&receipt(), credential(), &refused)
                .await
                .map(|_| ()),
            _ => authority
                .resolve_review_conversation(
                    SourceReviewResolutionRequest {
                        review: &receipt(),
                        response: &response,
                        checkpoint: &refused,
                    },
                    credential(),
                )
                .await
                .map(|_| ()),
        };
        assert!(result.is_err(), "{gate}");
        assert!(
            refused.states.lock().unwrap().last().unwrap()["gitlab"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            authority
                .review_conversation_state()
                .await
                .unwrap()
                .unwrap(),
            acknowledged
        );
        for _ in 0..2 {
            assert!(
                authority
                    .inspect_review_feedback(&receipt(), credential(), &RefuseReviewCheckpoint)
                    .await
                    .is_err()
            );
            assert!(
                authority
                    .request_merge(&receipt(), credential(), &RefuseReviewCheckpoint)
                    .await
                    .is_err()
            );
            assert!(
                authority
                    .resolve_review_conversation(
                        SourceReviewResolutionRequest {
                            review: &receipt(),
                            response: &response,
                            checkpoint: &RefuseReviewCheckpoint,
                        },
                        credential()
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
                acknowledged,
                "{gate}"
            );
        }

        let checkpoint = RecordingReviewCheckpoint::default();
        assert_eq!(
            authority
                .request_merge(&receipt(), credential(), &checkpoint)
                .await
                .unwrap(),
            SourceMergeRequestOutcome::ReviewBlocked
        );
        let feedback = authority
            .inspect_review_feedback(&receipt(), credential(), &checkpoint)
            .await
            .unwrap();
        assert_eq!(feedback.items.len(), 1);
        assert_eq!(feedback.conversations.len(), 1);
        let states = checkpoint.states.lock().unwrap().clone();
        assert_eq!(states.len(), 1);
        assert!(states[0]["gitlab"].as_array().unwrap().is_empty());
        assert_eq!(states[0]["replies"], acknowledged["replies"]);
        assert_eq!(
            authority
                .review_conversation_state()
                .await
                .unwrap()
                .unwrap(),
            states[0]
        );
        assert_eq!(checkpoint.states.lock().unwrap().len(), 1);
        assert_eq!(
            fixture.state.lock().unwrap().review.as_ref().unwrap()["blocking_discussions_resolved"],
            false
        );
        assert_eq!(
            fixture.state.lock().unwrap().conversation.as_ref().unwrap()["notes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(fixture.writes("/discussions/thread/notes"), 1);
        assert_eq!(fixture.writes("/merge"), 0);
    }
}
