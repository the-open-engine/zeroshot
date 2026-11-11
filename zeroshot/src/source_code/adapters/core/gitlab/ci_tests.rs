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
        assert_no_ci_diagnostics(&fixture);
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
            if status != Some("failed") {
                assert_no_ci_diagnostics(&fixture);
            }
        }
    }
}

fn assert_no_ci_diagnostics(fixture: &Fixture) {
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|(_, path, _)| {
                !path.contains("/pipelines/")
                    && !path.contains("/jobs/")
                    && !path.ends_with("/graphql")
            })
    );
}

fn failed_job(id: u64, name: &str, stage: &str, reason: Option<&str>) -> Value {
    json!({
        "id":id, "name":name, "stage":stage, "status":"failed", "failure_reason":reason,
        "pipeline":{"id":81,"project_id":11,"sha":HEAD,"status":"failed"},
    })
}

async fn failure_diagnostic(fixture: &Fixture) -> String {
    let observed = fixture
        .authority
        .inspect_review(&receipt(), credential())
        .await
        .unwrap();
    assert!(!observed.pull_request_ready);
    let SourceReviewState::Open {
        checks: SourceChecks::Failed { diagnostic },
    } = observed.state
    else {
        panic!("expected a failed required pipeline");
    };
    diagnostic
}

#[tokio::test]
async fn required_pipeline_failure_names_failed_jobs_and_labels_raw_log_tails() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    {
        let mut state = fixture.state.lock().unwrap();
        let mut passed = failed_job(93, "passing", "test", None);
        passed["status"] = json!("success");
        let mut running = failed_job(94, "running", "build", None);
        running["status"] = json!("running");
        let mut skipped = failed_job(95, "skipped", "deploy", None);
        skipped["status"] = json!("skipped");
        state.job_pages.insert(
            1,
            vec![
                failed_job(91, "unit tests", "test", Some("script_failure")),
                failed_job(92, "package", "build", Some("runner_system_failure")),
                passed,
                running,
                skipped,
            ],
        );
        state.job_logs.insert(
            91,
            b"setup succeeded\nassertion failed: expected 2, got 1\n".to_vec(),
        );
        state
            .job_logs
            .insert(92, b"runner lost\nno space left on device\n".to_vec());
    }
    let diagnostic = failure_diagnostic(&fixture).await;
    assert!(diagnostic.contains("unit tests (stage: test) failed: script_failure (job 91)"));
    assert!(diagnostic.contains("package (stage: build) failed: runner_system_failure (job 92)"));
    assert!(
        diagnostic.contains("--- GitLab job 91 log excerpt ---\nsetup succeeded\nassertion failed")
    );
    assert!(diagnostic.contains("--- GitLab job 92 log excerpt ---\nrunner lost\nno space left"));
    assert!(!diagnostic.contains("passing"));
    assert!(!diagnostic.contains("stage: deploy"));
    for known in [true, false] {
        assert_eq!(
            fixture
                .observe_review(known)
                .await
                .unwrap()
                .review
                .unwrap()
                .state,
            SourceReviewState::Open {
                checks: SourceChecks::Failed {
                    diagnostic: diagnostic.clone()
                }
            }
        );
    }
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|(_, path, _)| {
                !path.contains("/jobs/93/")
                    && !path.contains("/jobs/94/")
                    && !path.contains("/jobs/95/")
            })
    );
    assert_eq!(fixture.writes("/merge"), 0);
}

#[tokio::test]
async fn pipeline_configuration_error_is_reported_without_jobs_and_redacted() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    let basic = crate::native_v2_delivery::git_auth::encode_basic_credential(credential().expose());
    let mut pipeline = merge_request()["head_pipeline"].clone();
    pipeline["status"] = json!("failed");
    pipeline["yaml_errors"] = json!(format!(
        "jobs:build config must specify script\nsecret: test-private-token {basic}\u{0007}"
    ));
    fixture.response("GET", "/pipelines/81", (StatusCode::OK, pipeline));
    let diagnostic = failure_diagnostic(&fixture).await;
    assert!(
        diagnostic.contains("Pipeline configuration error: jobs:build config must specify script")
    );
    assert!(diagnostic.contains("[REDACTED]"));
    assert!(!diagnostic.contains("test-private-token"));
    assert!(!diagnostic.contains(&basic));
    assert!(!diagnostic.contains('\u{0007}'));
    assert!(!diagnostic.contains("log excerpt"));
}

#[tokio::test]
async fn jobless_configuration_failure_reports_graphql_errors_when_yaml_errors_is_null() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    fixture.state.lock().unwrap().review.as_mut().unwrap()["head_pipeline"]["yaml_errors"] =
        Value::Null;
    let message =
        "jobs unit-evidence config should implement the script:, run:, or trigger: keyword";
    fixture.response(
        "POST",
        "/api/graphql",
        (
            StatusCode::OK,
            pipeline_error_page(vec![json!({"content":message})], None),
        ),
    );
    let diagnostic = failure_diagnostic(&fixture).await;
    assert_eq!(
        diagnostic,
        format!("GitLab pipeline 81 failed\nPipeline configuration error: {message}")
    );
    assert!(!diagnostic.contains("log excerpt"));
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(method, path, _)| method == "POST" && path == "/api/graphql")
    );
}

#[tokio::test]
async fn pipeline_graphql_errors_are_paginated_bounded_and_redacted() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    let basic = crate::native_v2_delivery::git_auth::encode_basic_credential(credential().expose());
    {
        let mut state = fixture.state.lock().unwrap();
        state.pipeline_error_pages.insert(None, pipeline_error_page(vec![
            json!({"content":format!("invalid config test-private-token {basic}\n\u{0007}{}\u{001b}", "界".repeat(2000))}),
        ], Some("errors-page-2")));
        let mut messages = vec![json!({"content":"second configuration error"})];
        messages.extend(
            (0..10).map(|index| json!({"content":format!("{index}{}", "界".repeat(2000))})),
        );
        state.pipeline_error_pages.insert(
            Some("errors-page-2".into()),
            pipeline_error_page(messages, None),
        );
    }
    let diagnostic = failure_diagnostic(&fixture).await;
    assert!(diagnostic.contains("second configuration error"));
    assert!(diagnostic.contains("[diagnostic truncated]"));
    assert!(diagnostic.contains("[REDACTED]"));
    assert!(!diagnostic.contains("test-private-token"));
    assert!(!diagnostic.contains(&basic));
    assert!(diagnostic.chars().count() <= 8 * 1024);
    assert!(
        diagnostic
            .lines()
            .skip(1)
            .all(|line| line.chars().count() <= 1024 + "Pipeline configuration error: ".len())
    );
    assert!(diagnostic.chars().all(|c| !c.is_control() || c == '\n'));
    let state = fixture.state.lock().unwrap();
    let calls = state
        .calls
        .iter()
        .filter(|(_, path, _)| path.ends_with("/graphql"))
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].2["variables"]["after"], Value::Null);
    assert_eq!(calls[1].2["variables"]["after"], "errors-page-2");
}

#[tokio::test]
async fn graphql_requests_preserve_the_admitted_installation_prefix() {
    let fixture = Fixture::with_api_prefix("service/").await;
    let _: Value = fixture
        .authority
        .api
        .json(
            ApiRequest::graphql(
                "query { project { id } }",
                json!({
                    "project":REPOSITORY, "pipeline":"gid://gitlab/Ci::Pipeline/81", "after":null,
                }),
            ),
            credential(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.state.lock().unwrap().calls[0].1,
        "/service/api/graphql"
    );
}

#[tokio::test]
async fn rest_and_graphql_configuration_errors_are_reported_once() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    fixture.state.lock().unwrap().review.as_mut().unwrap()["head_pipeline"]["yaml_errors"] =
        json!("invalid job configuration");
    fixture.response(
        "POST",
        "/api/graphql",
        (
            StatusCode::OK,
            pipeline_error_page(
                vec![
                    json!({"content":"invalid job configuration"}),
                    json!({"content":""}),
                ],
                None,
            ),
        ),
    );
    let diagnostic = failure_diagnostic(&fixture).await;
    assert_eq!(diagnostic.matches("invalid job configuration").count(), 1);
    assert_eq!(
        diagnostic.matches("Pipeline configuration error:").count(),
        1
    );
}

#[tokio::test]
async fn graphql_error_evidence_must_be_complete_and_bound_to_the_captured_pipeline() {
    let good = pipeline_error_page(Vec::new(), None);
    let mut wrong_project = good.clone();
    wrong_project["data"]["project"]["id"] = json!("gid://gitlab/Project/22");
    let mut wrong_pipeline = good.clone();
    wrong_pipeline["data"]["project"]["pipeline"]["id"] = json!("gid://gitlab/Ci::Pipeline/82");
    let mut wrong_sha = good.clone();
    wrong_sha["data"]["project"]["pipeline"]["sha"] = json!(MERGED);
    let mut partial = good.clone();
    partial["errors"] = json!([{"message":"test-private-token error detail"}]);
    let mut null_connection = good.clone();
    null_connection["data"]["project"]["pipeline"]["errorMessages"] = Value::Null;
    let mut missing_cursor = good.clone();
    missing_cursor["data"]["project"]["pipeline"]["errorMessages"]["pageInfo"]["hasNextPage"] =
        json!(true);
    for response in [
        wrong_project,
        wrong_pipeline,
        wrong_sha,
        partial,
        null_connection,
        missing_cursor,
        json!({"data":{"project":null}}),
        pipeline_error_page(Vec::new(), Some("repeated-cursor")),
    ] {
        let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
        fixture.response("POST", "/api/graphql", (StatusCode::OK, response));
        let error = fixture
            .authority
            .inspect_review(&receipt(), credential())
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("test-private-token"));
        assert_eq!(fixture.writes("/merge"), 0);
    }
}

#[tokio::test]
async fn numeric_repository_identity_uses_the_canonical_graphql_project_path() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    let mut review = receipt();
    review.repository = "11".into();
    fixture
        .authority
        .inspect_review(&review, credential())
        .await
        .unwrap();
    let state = fixture.state.lock().unwrap();
    let (_, _, request) = state
        .calls
        .iter()
        .find(|(_, path, _)| path.ends_with("/graphql"))
        .unwrap();
    assert_eq!(request["variables"]["project"], REPOSITORY);
}

#[tokio::test]
async fn failed_trigger_jobs_are_paginated_and_keep_details_without_logs() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    {
        let mut state = fixture.state.lock().unwrap();
        state.trigger_job_pages.insert(
            1,
            (1..101)
                .map(|id| {
                    let mut job = failed_job(id, "successful trigger", "deploy", None);
                    job["status"] = json!("success");
                    job
                })
                .collect(),
        );
        state.trigger_job_pages.insert(
            2,
            vec![failed_job(
                101,
                "child pipeline",
                "test",
                Some("downstream_bridge_failure"),
            )],
        );
    }
    let diagnostic = failure_diagnostic(&fixture).await;
    assert!(
        diagnostic
            .contains("child pipeline (stage: test) failed: downstream_bridge_failure (job 101)")
    );
    assert!(diagnostic.contains("--- GitLab job 101 log excerpt ---\nGitLab job log unavailable:"));
    assert!(!diagnostic.contains("successful trigger"));
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(_, path, _)| path.ends_with("/trigger_jobs"))
            .count(),
        2
    );
}

#[tokio::test]
async fn regular_and_trigger_failures_share_the_job_log_budget() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    {
        let mut state = fixture.state.lock().unwrap();
        state.job_pages.insert(
            1,
            (91..95)
                .map(|id| failed_job(id, &format!("regular-{id}"), "test", Some("script_failure")))
                .collect(),
        );
        state.trigger_job_pages.insert(
            1,
            (95..101)
                .map(|id| {
                    failed_job(
                        id,
                        &format!("trigger-{id}"),
                        "test",
                        Some("downstream_bridge_failure"),
                    )
                })
                .collect(),
        );
        for id in 91..101 {
            state
                .job_logs
                .insert(id, format!("failed job {id}").into_bytes());
        }
    }
    let diagnostic = failure_diagnostic(&fixture).await;
    assert!(diagnostic.contains("regular-91 (stage: test) failed: script_failure"));
    assert!(diagnostic.contains("trigger-100 (stage: test) failed: downstream_bridge_failure"));
    assert!(diagnostic.contains("Log excerpts limited to 8 of 10 failed jobs"));
    assert_eq!(diagnostic.matches("log excerpt ---").count(), 8);
    assert!(diagnostic.contains("--- GitLab job 98 log excerpt ---\nfailed job 98"));
    assert!(!diagnostic.contains("--- GitLab job 99 log excerpt ---"));
}

#[tokio::test]
async fn unavailable_job_logs_preserve_job_details_and_other_excerpts() {
    for status in [
        StatusCode::NOT_FOUND,
        StatusCode::FORBIDDEN,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
        {
            let mut state = fixture.state.lock().unwrap();
            state.job_pages.insert(
                1,
                vec![
                    failed_job(91, "tests", "test", Some("script_failure")),
                    failed_job(92, "package", "build", None),
                ],
            );
            state.job_logs.insert(92, b"compilation failed".to_vec());
        }
        fixture.response(
            "GET",
            "/jobs/91/trace",
            (status, json!({"message":"log erased test-private-token"})),
        );
        let diagnostic = failure_diagnostic(&fixture).await;
        assert!(diagnostic.contains("tests (stage: test) failed: script_failure"));
        assert!(diagnostic.contains("package (stage: build) failed: reason unavailable"));
        assert!(diagnostic.contains("GitLab job log unavailable:"));
        assert!(diagnostic.contains("--- GitLab job 92 log excerpt ---\ncompilation failed"));
        assert!(!diagnostic.contains("test-private-token"));
    }
}

#[tokio::test]
async fn large_logs_are_bounded_redacted_and_fairly_shared_with_explicit_omissions() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    let basic = crate::native_v2_delivery::git_auth::encode_basic_credential(credential().expose());
    {
        let mut state = fixture.state.lock().unwrap();
        state.job_pages.insert(
            1,
            (91..101)
                .map(|id| failed_job(id, &format!("tests-{id}"), "test", Some("script_failure")))
                .collect(),
        );
        for id in 91..101 {
            // Exceeds the JSON response bound, but is within GitHub's job-log capture bound.
            state.job_logs.insert(id, format!(
                "beginning must disappear{}\nassertion failed in job {id}\n\u{001b}[31mtest-private-token {basic}\u{0000}\n",
                "界".repeat(400_000),
            ).into_bytes());
        }
    }
    let diagnostic = failure_diagnostic(&fixture).await;
    assert!(diagnostic.chars().count() <= 64 * 1024);
    assert!(diagnostic.contains("tests-100 (stage: test) failed: script_failure"));
    assert!(diagnostic.contains("Log excerpts limited to 8 of 10 failed jobs"));
    assert_eq!(
        diagnostic.matches("[earlier log output truncated]").count(),
        8
    );
    assert_eq!(diagnostic.matches("log excerpt ---").count(), 8);
    for id in 91..99 {
        assert!(diagnostic.contains(&format!("--- GitLab job {id} log excerpt ---")));
        assert!(diagnostic.contains(&format!("assertion failed in job {id}")));
    }
    assert!(!diagnostic.contains("beginning must disappear"));
    assert!(!diagnostic.contains("test-private-token"));
    assert!(!diagnostic.contains(&basic));
    assert!(
        diagnostic
            .chars()
            .all(|c| !c.is_control() || matches!(c, '\n' | '\t'))
    );
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(_, path, _)| path.ends_with("/trace"))
            .count(),
        8
    );
}

#[tokio::test]
async fn failed_job_summary_respects_github_item_and_total_diagnostic_limits() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    fixture.state.lock().unwrap().job_pages.insert(
        1,
        (91..103)
            .map(|id| failed_job(id, &"界".repeat(2000), "test", Some("script_failure")))
            .collect(),
    );
    let diagnostic = failure_diagnostic(&fixture).await;
    let summary = diagnostic.split("\n--- GitLab job").next().unwrap();
    assert_eq!(summary.chars().count(), 8 * 1024);
    assert!(summary.ends_with("[diagnostic truncated]"));
    assert!(
        summary
            .lines()
            .skip(1)
            .all(|line| line.chars().count() <= 1026)
    );
    assert_eq!(diagnostic.matches("log excerpt ---").count(), 8);
    assert!(diagnostic.chars().count() <= 64 * 1024);
}

#[tokio::test]
async fn failed_jobs_are_collected_from_every_pipeline_job_page() {
    let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
    {
        let mut state = fixture.state.lock().unwrap();
        state.job_pages.insert(
            1,
            (1..101)
                .map(|id| {
                    let mut job = failed_job(id, "passed", "test", None);
                    job["status"] = json!("success");
                    job
                })
                .collect(),
        );
        state.job_pages.insert(
            2,
            vec![failed_job(
                101,
                "last-page failure",
                "deploy",
                Some("script_failure"),
            )],
        );
        state.job_logs.insert(101, b"deployment failed".to_vec());
    }
    let diagnostic = failure_diagnostic(&fixture).await;
    assert!(diagnostic.contains("last-page failure (stage: deploy) failed: script_failure"));
    assert!(diagnostic.contains("--- GitLab job 101 log excerpt ---\ndeployment failed"));
    assert!(!diagnostic.contains("passed"));
}

#[tokio::test]
async fn diagnostic_evidence_must_match_the_failed_pipeline_identity() {
    for field in ["id", "sha", "project_id"] {
        for job_response in [false, true] {
            let fixture = ci_fixture(Some(json!(true)), Some("failed")).await;
            let mut job = failed_job(91, "tests", "test", Some("script_failure"));
            let mut pipeline = job["pipeline"].clone();
            pipeline[field] = match field {
                "sha" => json!(MERGED),
                _ => json!(999),
            };
            if job_response {
                job["pipeline"] = pipeline;
                fixture.state.lock().unwrap().job_pages.insert(1, vec![job]);
            } else {
                fixture.response("GET", "/pipelines/81", (StatusCode::OK, pipeline));
            }
            let error = fixture
                .authority
                .inspect_review(&receipt(), credential())
                .await
                .unwrap_err();
            assert!(error.to_string().contains("changed pipeline identity"));
            assert_eq!(fixture.writes("/merge"), 0);
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
