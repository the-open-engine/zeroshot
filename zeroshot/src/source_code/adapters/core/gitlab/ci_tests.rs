use super::*;

async fn ci_fixture(required: Option<Value>, status: Option<&str>) -> Fixture {
    let fixture = Fixture::new().await;
    fixture.open_review();
    {
        let mut state = fixture.state.lock().unwrap();
        if let Some(required) = required {
            state.project["only_allow_merge_if_pipeline_succeeds"] = required;
        } else {
            state
                .project
                .as_object_mut()
                .unwrap()
                .remove("only_allow_merge_if_pipeline_succeeds");
        }
        let review = state.review.as_mut().unwrap();
        if let Some(status) = status {
            review["head_pipeline"]["status"] = json!(status);
        } else {
            review["head_pipeline"] = Value::Null;
        }
    }
    fixture
}

async fn assert_ci_policy(fixture: &Fixture, expected: SourceChecks) {
    let observed = fixture
        .authority
        .inspect_review(&receipt(), credential())
        .await
        .unwrap();
    assert_eq!(
        observed.state,
        SourceReviewState::Open {
            checks: expected.clone()
        }
    );
    let can_merge = matches!(expected, SourceChecks::Passed | SourceChecks::NotRequired);
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential())
            .await
            .unwrap(),
        if can_merge {
            SourceMergeRequestOutcome::Accepted
        } else {
            SourceMergeRequestOutcome::Pending
        }
    );
    assert_eq!(fixture.writes("/merge"), usize::from(can_merge));
    if can_merge {
        {
            let state = fixture.state.lock().unwrap();
            let (_, _, body) = state
                .calls
                .iter()
                .find(|(method, path, _)| method == "PUT" && path.ends_with("/merge"))
                .unwrap();
            assert_eq!(body, &json!({"sha":HEAD,"auto_merge":false}));
        }
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
async fn optional_ci_never_gates_readiness_or_merge_and_is_never_reported_passed() {
    for status in [
        None,
        Some("pending"),
        Some("running"),
        Some("success"),
        Some("failed"),
        Some("skipped"),
        Some("canceled"),
        Some("manual"),
        Some("future_status"),
    ] {
        let fixture = ci_fixture(Some(json!(false)), status).await;
        assert_ci_policy(&fixture, SourceChecks::NotRequired).await;
    }
}

#[tokio::test]
async fn required_and_unknown_ci_policy_require_captured_head_pipeline_evidence() {
    for required in [Some(json!(true)), None, Some(Value::Null)] {
        for status in [
            None,
            Some("pending"),
            Some("running"),
            Some("success"),
            Some("failed"),
            Some("skipped"),
            Some("canceled"),
            Some("future_status"),
        ] {
            let fixture = ci_fixture(required.clone(), status).await;
            let expected = match status {
                Some("success") => SourceChecks::Passed,
                Some("failed") => SourceChecks::Failed {
                    diagnostic: "GitLab pipeline 81 failed".into(),
                },
                _ => SourceChecks::Pending,
            };
            assert_ci_policy(&fixture, expected).await;
        }
    }
}

#[tokio::test]
async fn skipped_required_ci_still_requires_explicit_project_permission() {
    for allowed in [
        Some(json!(true)),
        Some(json!(false)),
        Some(Value::Null),
        None,
    ] {
        let fixture = ci_fixture(Some(json!(true)), Some("skipped")).await;
        {
            let mut state = fixture.state.lock().unwrap();
            if let Some(allowed) = &allowed {
                state.project["allow_merge_on_skipped_pipeline"] = allowed.clone();
            } else {
                state
                    .project
                    .as_object_mut()
                    .unwrap()
                    .remove("allow_merge_on_skipped_pipeline");
            }
        }
        let expected = if allowed == Some(json!(true)) {
            SourceChecks::Passed
        } else {
            SourceChecks::Pending
        };
        assert_ci_policy(&fixture, expected).await;
    }
}

#[tokio::test]
async fn approval_handoff_uses_required_ci_policy_without_authorizing_merge() {
    for required in [Some(json!(false)), Some(json!(true)), None] {
        for status in [None, Some("running"), Some("success"), Some("failed")] {
            for merge_status in ["not_approved", "requested_changes"] {
                let fixture = ci_fixture(required.clone(), status).await;
                fixture.state.lock().unwrap().review.as_mut().unwrap()["detailed_merge_status"] =
                    json!(merge_status);
                let observed = fixture
                    .authority
                    .inspect_review(&receipt(), credential())
                    .await
                    .unwrap();
                let optional = required == Some(json!(false));
                assert_eq!(
                    observed.pull_request_ready,
                    optional || status == Some("success")
                );
                let checks = if optional {
                    SourceChecks::NotRequired
                } else {
                    match status {
                        Some("success") => SourceChecks::Passed,
                        Some("failed") => SourceChecks::Failed {
                            diagnostic: "GitLab pipeline 81 failed".into(),
                        },
                        _ => SourceChecks::Pending,
                    }
                };
                assert_eq!(observed.state, SourceReviewState::Open { checks });
                assert_eq!(
                    fixture
                        .authority
                        .request_merge(&receipt(), credential())
                        .await
                        .unwrap(),
                    SourceMergeRequestOutcome::Pending
                );
                assert_eq!(fixture.writes("/merge"), 0);
            }
        }
    }
}

#[tokio::test]
async fn optional_ci_preserves_non_ci_readiness_and_merge_gates() {
    for (review_patch, project_patch) in [
        (json!({"draft":true}), json!({})),
        (json!({"has_conflicts":true}), json!({})),
        (json!({"blocking_discussions_resolved":false}), json!({})),
        (json!({"blocking_discussions_resolved":null}), json!({})),
        (json!({"merge_when_pipeline_succeeds":true}), json!({})),
        (json!({"state":"locked"}), json!({})),
        (json!({"detailed_merge_status":"checking"}), json!({})),
        (json!({"detailed_merge_status":"future_status"}), json!({})),
        (json!({"detailed_merge_status":"conflict"}), json!({})),
        (json!({"detailed_merge_status":"need_rebase"}), json!({})),
        (json!({}), json!({"merge_trains_enabled":true})),
    ] {
        let fixture = ci_fixture(Some(json!(false)), Some("failed")).await;
        let conflict = matches!(
            review_patch["detailed_merge_status"].as_str(),
            Some("conflict" | "need_rebase")
        );
        {
            let mut state = fixture.state.lock().unwrap();
            state
                .review
                .as_mut()
                .unwrap()
                .as_object_mut()
                .unwrap()
                .extend(review_patch.as_object().unwrap().clone());
            state
                .project
                .as_object_mut()
                .unwrap()
                .extend(project_patch.as_object().unwrap().clone());
        }
        let observed = fixture
            .authority
            .inspect_review(&receipt(), credential())
            .await
            .unwrap();
        assert!(!observed.pull_request_ready);
        assert_eq!(
            observed.state,
            if conflict {
                SourceReviewState::Conflict
            } else {
                SourceReviewState::Open {
                    checks: SourceChecks::Pending,
                }
            }
        );
        assert_eq!(
            fixture
                .authority
                .request_merge(&receipt(), credential())
                .await
                .unwrap(),
            if conflict {
                SourceMergeRequestOutcome::Conflict
            } else {
                SourceMergeRequestOutcome::Pending
            }
        );
        assert_eq!(fixture.writes("/merge"), 0);
    }
}

#[tokio::test]
async fn optional_ci_cannot_bypass_project_review_or_captured_head_identity() {
    for (field, value) in [
        ("iid", json!(18)),
        ("source_project_id", json!(22)),
        ("target_project_id", json!(22)),
        ("source_branch", json!("other")),
        ("target_branch", json!("other")),
        ("sha", json!(MERGED)),
        ("sha", Value::Null),
    ] {
        let fixture = ci_fixture(Some(json!(false)), Some("failed")).await;
        fixture.state.lock().unwrap().review.as_mut().unwrap()[field] = value;
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
                .request_merge(&receipt(), credential())
                .await
                .is_err()
        );
        assert_eq!(fixture.writes("/merge"), 0);
    }
    let fixture = ci_fixture(Some(json!(false)), Some("failed")).await;
    fixture.state.lock().unwrap().project["path_with_namespace"] = json!("other/project");
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
            .request_merge(&receipt(), credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/merge"), 0);
}
