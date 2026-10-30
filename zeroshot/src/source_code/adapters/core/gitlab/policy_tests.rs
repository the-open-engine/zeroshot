use super::*;

#[tokio::test]
async fn project_identity_cache_is_bounded_and_refreshes_existing_scopes() {
    let fixture = Fixture::new().await;
    for index in 0..9 {
        let repository = format!("group/project-{index}");
        let mut observed = project();
        observed["id"] = json!(index + 1);
        observed["path_with_namespace"] = json!(repository);
        observed["web_url"] = json!(format!("{ORIGIN}/{repository}"));
        fixture.state.lock().unwrap().project = observed;
        let result = fixture.authority.project(&repository, credential()).await;
        if index < 8 {
            assert_eq!(result.unwrap().path_with_namespace, repository);
        } else {
            assert!(result.unwrap_err().to_string().contains("scope exceeded"));
        }
    }
    let mut existing = project();
    existing["id"] = json!(1);
    existing["path_with_namespace"] = json!("group/project-0");
    existing["web_url"] = json!(format!("{ORIGIN}/group/project-0"));
    fixture.state.lock().unwrap().project = existing;
    fixture
        .authority
        .project("group/project-0", credential())
        .await
        .unwrap();
    assert_eq!(fixture.authority.projects.lock().unwrap().len(), 8);
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
async fn git_effects_require_the_admitted_transport_before_mutating_a_workspace() {
    let fixture = Fixture::new().await;
    fixture.open_review();
    let workspace = tempfile::tempdir().unwrap();
    let request = request();
    let review = receipt();
    let push = SourcePushRequest {
        workspace: workspace.path().to_owned(),
        target: request.target.clone(),
        head_branch: request.head_branch.clone(),
        head_revision: request.head_revision.clone(),
    };
    let conflict = SourceConflictRequest {
        workspace: workspace.path().to_owned(),
        review: review.clone(),
    };
    let target = SourceTargetReconciliation {
        workspace: workspace.path(),
        target: &request.target,
        commit_message: "Integrate target",
    };
    let head = SourceHeadReconciliation {
        workspace: workspace.path(),
        published: &review,
        observed: &review,
        commit_message: "Reconcile head",
        authorized_update: false,
        adopting_existing: false,
    };
    for result in [
        fixture.authority.push_branch(&push, credential()).await,
        fixture
            .authority
            .reconcile_delivery_target(target, credential())
            .await
            .map(|_| ()),
        fixture
            .authority
            .reconcile_delivery_head(head, credential())
            .await
            .map(|_| ()),
        fixture
            .authority
            .materialize_merge_conflict(&conflict, credential())
            .await
            .map(|_| ()),
    ] {
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("requires the admitted Git transport")
        );
    }
    assert_eq!(std::fs::read_dir(workspace.path()).unwrap().count(), 0);
    fixture.state.lock().unwrap().review.as_mut().unwrap()["detailed_merge_status"] =
        json!("need_rebase");
    assert!(
        fixture
            .authority
            .materialize_merge_conflict(&conflict, credential())
            .await
            .is_err()
    );
    fixture.response(
        "GET",
        "/repository/branches/main",
        (StatusCode::NOT_FOUND, Value::Null),
    );
    assert!(
        fixture
            .authority
            .reconcile_delivery_target(target, credential())
            .await
            .unwrap_err()
            .to_string()
            .contains("target branch is absent")
    );
    assert!(
        fixture
            .authority
            .materialize_merge_conflict(&conflict, credential())
            .await
            .unwrap_err()
            .to_string()
            .contains("target branch is absent")
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
async fn branch_only_observation_distinguishes_absence_from_invalid_identity_and_denial() {
    let fixture = Fixture::new().await;
    let request = request();
    let read = SourceDeliveryRead {
        target: &request.target,
        head_branch: &request.head_branch,
        known_review: None,
        include_review: false,
    };
    let observed = fixture
        .authority
        .observe_delivery(read, credential())
        .await
        .unwrap();
    assert_eq!(observed.head_revision.as_deref(), Some(HEAD));
    assert!(observed.review.is_none());
    for (status, response, absent) in [
        (StatusCode::NOT_FOUND, Value::Null, true),
        (
            StatusCode::OK,
            json!({"name":"wrong","commit":{"id":HEAD}}),
            false,
        ),
        (
            StatusCode::OK,
            json!({"name":"zeroshot/test","commit":{"id":"invalid"}}),
            false,
        ),
        (StatusCode::FORBIDDEN, json!({"message":"denied"}), false),
    ] {
        fixture.response(
            "GET",
            "/repository/branches/zeroshot%2Ftest",
            (status, response),
        );
        let result = fixture.authority.observe_delivery(read, credential()).await;
        if absent {
            let snapshot = result.unwrap();
            assert!(snapshot.head_revision.is_none() && snapshot.review.is_none());
        } else {
            assert!(result.is_err());
        }
    }
    assert!(
        !fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(_, path, _)| path.ends_with("/merge_requests"))
    );
}

#[tokio::test]
async fn optional_missing_skipped_and_failed_pipelines_keep_distinct_policy_outcomes() {
    let fixture = Fixture::new().await;
    for (pipeline, required, skipped, expected) in [
        (None, false, false, SourceChecks::NotRequired),
        (None, true, false, SourceChecks::Pending),
        (Some("skipped"), true, true, SourceChecks::Passed),
        (Some("skipped"), true, false, SourceChecks::Pending),
        (Some("canceled"), true, false, SourceChecks::Pending),
        (
            Some("failed"),
            true,
            false,
            SourceChecks::Failed {
                diagnostic: "GitLab pipeline 81 failed".into(),
            },
        ),
    ] {
        let mut mr = merge_request();
        mr["head_pipeline"] = pipeline.map_or(Value::Null, |status| {
            let mut pipeline = merge_request()["head_pipeline"].clone();
            pipeline["status"] = json!(status);
            pipeline
        });
        {
            let mut state = fixture.state.lock().unwrap();
            state.review = Some(mr);
            state.project["only_allow_merge_if_pipeline_succeeds"] = json!(required);
            state.project["allow_merge_on_skipped_pipeline"] = json!(skipped);
        }
        assert_eq!(
            fixture
                .authority
                .inspect_review(&receipt(), credential())
                .await
                .unwrap()
                .state,
            SourceReviewState::Open { checks: expected }
        );
    }
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn malformed_pipeline_identity_cannot_authorize_a_merge() {
    let fixture = Fixture::new().await;
    for (field, value) in [
        ("id", json!(0)),
        ("project_id", json!(22)),
        ("sha", json!("not-a-commit")),
    ] {
        let mut mr = merge_request();
        mr["head_pipeline"][field] = value;
        fixture.state.lock().unwrap().review = Some(mr);
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
    }
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn closed_locked_and_conflicting_reviews_never_become_mergeable() {
    let fixture = Fixture::new().await;
    for (state, status, expected) in [
        ("closed", "mergeable", SourceReviewState::Closed),
        (
            "locked",
            "mergeable",
            SourceReviewState::Open {
                checks: SourceChecks::Pending,
            },
        ),
        ("opened", "conflict", SourceReviewState::Conflict),
        ("opened", "need_rebase", SourceReviewState::Conflict),
    ] {
        let mut mr = merge_request();
        mr["state"] = json!(state);
        mr["detailed_merge_status"] = json!(status);
        fixture.state.lock().unwrap().review = Some(mr);
        assert_eq!(
            fixture
                .authority
                .inspect_review(&receipt(), credential())
                .await
                .unwrap()
                .state,
            expected
        );
        if matches!(status, "conflict" | "need_rebase") {
            assert_eq!(
                fixture
                    .authority
                    .request_merge(&receipt(), credential())
                    .await
                    .unwrap(),
                SourceMergeRequestOutcome::Conflict
            );
        } else {
            assert_eq!(
                fixture
                    .authority
                    .request_merge(&receipt(), credential())
                    .await
                    .unwrap(),
                SourceMergeRequestOutcome::Pending
            );
        }
    }
    fixture.state.lock().unwrap().review.as_mut().unwrap()["state"] = json!("future-state");
    assert!(
        fixture
            .authority
            .inspect_review(&receipt(), credential())
            .await
            .is_err()
    );
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn already_integrated_squash_revision_is_confirmed_without_another_merge_effect() {
    let fixture = Fixture::new().await;
    let mut mr = merge_request();
    mr["state"] = json!("merged");
    mr["squash_commit_sha"] = json!(MERGED);
    fixture.state.lock().unwrap().review = Some(mr);
    assert_eq!(
        fixture
            .authority
            .request_merge(&receipt(), credential())
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
            merge_revision: MERGED.into()
        }
    );
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
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn instance_links_require_cached_project_identity_and_canonical_review_ids() {
    let fixture = Fixture::new().await;
    assert!(fixture.authority.review_links(&receipt()).is_err());
    fixture
        .authority
        .project(REPOSITORY, credential())
        .await
        .unwrap();
    assert_eq!(
        fixture.authority.forge_credential_field(),
        Some("GITLAB_TOKEN")
    );
    assert!(!format!("{:?}", fixture.authority).contains("test-private-token"));
    for (id, head) in [("0", HEAD), ("017", HEAD), ("17", "not-a-commit")] {
        let mut invalid = receipt();
        invalid.review_id = id.into();
        invalid.head_revision = head.into();
        assert!(fixture.authority.review_links(&invalid).is_err());
    }
    let mut missing_base = destination();
    missing_base.base_url = None;
    assert!(!valid_comment_permalink(
        &missing_base,
        "1",
        &format!("{}#note_1", missing_base.url)
    ));
}
