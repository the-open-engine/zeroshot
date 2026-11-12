use super::*;

#[tokio::test]
async fn resumed_delivery_reports_saved_publication_when_the_first_forge_read_fails() {
    for published in [false, true] {
        let repo = TempRepo::delivery();
        let branch = "feature/recovered-publication";
        git(&repo.workspace, &["checkout", "-b", branch]);
        crate::native_v2_candidate::test_support::commit_all(&repo.workspace, "committed work");
        git(&repo.workspace, &["push", "origin", branch]);
        let head = git_output(&repo.workspace, &["rev-parse", "HEAD"]);
        let authority = Arc::new(FakeGitHub::new(
            repo.remote.clone(),
            Script::PreflightRefusedAfterTransientRead,
        ));
        authority
            .reviews
            .lock()
            .assert_value()
            .push(SourceReviewRequest {
                include_terminal_reviews: false,
                known_review: None,
                target: target(&repo),
                head_branch: branch.to_owned(),
                head_revision: head.clone(),
                title: "Existing review".to_owned(),
                description: "Committed work.".to_owned(),
                source_issue: None,
            });
        let saved = SourceReviewReceipt {
            review_id: "17".to_owned(),
            repository: "acme/project".to_owned(),
            target_branch: "main".to_owned(),
            head_branch: branch.to_owned(),
            head_revision: head.clone(),
        };
        let adapter = Arc::new(
            NativeV2DeliveryAdapter::new(
                NativeV2DeliveryConfig {
                    local_branch: Some(branch.to_owned()),
                    delivery_run_id: RunId::new("recovered-publication"),
                    adopt_existing_delivery: true,
                    git_identity: None,
                    commit_identity: None,
                    commit_co_author: None,
                    workspace: repo.workspace.clone(),
                    git_program: "/usr/bin/git".into(),
                    target: target(&repo),
                    poll: DeliveryPollPolicy::new(1, Duration::ZERO).assert_value(),
                },
                authority.clone(),
            )
            .with_recovered_review(Some(contract::RecoveredReview {
                review: saved,
                published,
            })),
        );
        let request = || DeliveryRunRequest {
            repo: &repo,
            attempts: 1,
            mode: DeliveryMode::PullRequestV2,
            run_id: "recovered-publication",
            refresh: None,
        };
        let failed = run_with_adapter(request(), adapter.clone()).await;
        assert_eq!(
            failed.outcome,
            WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)
        );
        assert_eq!(authority.delivery_reads.load(Ordering::SeqCst), 1);
        assert_eq!(authority.target_reconciliations.load(Ordering::SeqCst), 0);
        assert_eq!(authority.head_updates.load(Ordering::SeqCst), 0);
        let diagnostic = failed
            .output
            .iter()
            .rfind(|output| output.diagnostic == Some(FailureDiagnosticKind::Failure))
            .assert_value();
        for expected in [
            "Already published",
            "acme/project#17",
            "main",
            branch,
            &head,
        ] {
            assert!(diagnostic.text.contains(expected), "{}", diagnostic.text);
        }
        assert!(
            !diagnostic
                .text
                .contains("No publication has been confirmed")
        );
        assert!(!diagnostic.text.contains("test-token"));
        assert_eq!(git_output(&repo.workspace, &["rev-parse", "HEAD"]), head);
        assert_eq!(git_output(&repo.remote, &["rev-parse", branch]), head);

        let resumed = run_with_adapter(request(), adapter).await;
        assert_delivery_signal(&resumed.outcome, DELIVERY_READY_LABEL);
        assert_eq!(
            authority.target_reconciliations.load(Ordering::SeqCst),
            usize::from(!published),
            "restored diagnostics must not promote an observed review to completed publication",
        );
    }
}

#[tokio::test]
async fn named_preflight_refusals_redact_credentials_even_when_they_match_the_branch() {
    let repo = TempRepo::delivery();
    let (authority, adapter) = scripted_adapter(
        &repo,
        Script::NoCi,
        2,
        DeliveryLineage::original("secret-branch"),
    );
    let request = || DeliveryRunRequest {
        repo: &repo,
        attempts: 2,
        mode: DeliveryMode::PullRequest,
        run_id: "secret-branch",
        refresh: None,
    };
    let opened = run_with_adapter(request(), adapter).await.outcome;
    assert_delivery_signal(&opened, DELIVERY_OPENED_LABEL);
    let fresh = retained_adapter(
        &repo,
        authority,
        DeliveryPollPolicy::new(2, Duration::ZERO).assert_value(),
        DeliveryLineage::original("secret-branch"),
    );
    let branch = delivery_head_branch(&RunId::new("secret-branch"));
    let refused = run_with_adapter_token(
        request(),
        fresh,
        PullRequestFeedback::Consider,
        Some(&branch),
    )
    .await;
    assert_eq!(
        refused.outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)
    );
    for output in refused.output {
        assert!(!output.text.contains(&branch), "{}", output.text);
    }
}

#[tokio::test]
async fn named_refusals_redact_native_and_declared_git_credentials_without_node_connections() {
    use openengine_cluster_protocol::GitTransportCredentials;

    for native in [false, true] {
        let repo = TempRepo::delivery();
        let (authority, adapter) = scripted_adapter(
            &repo,
            Script::NoCi,
            2,
            DeliveryLineage::original("native-secret-branch"),
        );
        let request = || DeliveryRunRequest {
            repo: &repo,
            attempts: 2,
            mode: DeliveryMode::PullRequest,
            run_id: "native-secret-branch",
            refresh: None,
        };
        assert_delivery_signal(
            &run_with_adapter(request(), adapter).await.outcome,
            DELIVERY_OPENED_LABEL,
        );
        let branch = delivery_head_branch(&RunId::new("native-secret-branch"));
        let fresh = retained_adapter(
            &repo,
            authority,
            DeliveryPollPolicy::new(2, Duration::ZERO).assert_value(),
            DeliveryLineage::original("native-secret-branch"),
        );
        let fresh = (*fresh)
            .clone()
            .with_trusted_github_token(Some(Arc::from("test-token")));
        let fresh = if native {
            fresh.with_local_git_environment(Some(BTreeMap::from([
                ("GIT_PASSWORD".to_owned(), branch.clone()),
                ("PUBLIC_VALUE".to_owned(), "main".to_owned()),
            ])))
        } else {
            fresh.with_git_credentials(Some(GitTransportCredentials::Https {
                username: "native-user".to_owned(),
                password: branch.clone(),
            }))
        };
        let refused = run_with_adapter_token(
            request(),
            Arc::new(fresh),
            PullRequestFeedback::Consider,
            None,
        )
        .await;
        assert_eq!(
            refused.outcome,
            WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)
        );
        for output in &refused.output {
            assert!(!output.text.contains(&branch), "{}", output.text);
        }
        let terminal = refused
            .output
            .iter()
            .find(|output| output.diagnostic == Some(FailureDiagnosticKind::Failure))
            .assert_value();
        for expected in ["Already published", "[REDACTED]", "main", "acme/project#17"] {
            assert!(terminal.text.contains(expected), "{}", terminal.text);
        }
    }
}

#[tokio::test]
async fn failed_delivery_names_confirmed_publication_before_and_after_review_creation() {
    for script in [Script::PolicyForbidden, Script::ReviewIdentityMismatch] {
        let repo = TempRepo::delivery();
        let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), script));
        let execution = run_delivery_execution(
            DeliveryRunRequest {
                repo: &repo,
                attempts: 2,
                mode: DeliveryMode::PullRequestV2,
                run_id: "publication-diagnostic",
                refresh: None,
            },
            authority,
        )
        .await;
        let branch = delivery_head_branch(&RunId::new("publication-diagnostic"));
        let head = git_output(&repo.remote, &["rev-parse", &branch]);
        let final_error = execution
            .output
            .iter()
            .rfind(|output| output.diagnostic == Some(FailureDiagnosticKind::Failure))
            .assert_value();
        for expected in ["Already published", "acme/project", "main", &branch, &head] {
            assert!(final_error.text.contains(expected), "{}", final_error.text);
        }
        let review = if matches!(script, Script::PolicyForbidden) {
            "review acme/project#17"
        } else {
            "review not confirmed"
        };
        assert!(final_error.text.contains(review), "{}", final_error.text);
        assert!(!final_error.text.contains("test-token"));
    }
}

#[tokio::test]
async fn repair_push_updates_diagnostics_without_advancing_withdrawal_authority() {
    let repo = TempRepo::delivery();
    let (authority, adapter) = scripted_adapter(
        &repo,
        Script::CiFailed,
        2,
        DeliveryLineage::original("repair-publication"),
    );
    let request = || DeliveryRunRequest {
        repo: &repo,
        attempts: 2,
        mode: DeliveryMode::MergeV3,
        run_id: "repair-publication",
        refresh: None,
    };
    let initial = run_with_adapter(request(), adapter.clone()).await.outcome;
    let original_head = assert_delivery_signal(&initial, DELIVERY_CI_FAILED_LABEL)["headRevision"]
        .as_str()
        .assert_value()
        .to_owned();
    fs::write(repo.workspace.join("result.txt"), "repaired candidate\n").assert_value();
    authority.reject_review_sync.store(true, Ordering::SeqCst);
    authority.queue_live.store(true, Ordering::SeqCst);
    authority.withdrawal_allowed.store(false, Ordering::SeqCst);
    let mut handle = start_with_adapter_token(
        request(),
        adapter,
        PullRequestFeedback::Consider,
        Some("test-token"),
    )
    .await;
    let error = handle.completion().await.unwrap_err();
    let branch = delivery_head_branch(&RunId::new("repair-publication"));
    let repaired_head = git_output(&repo.remote, &["rev-parse", &branch]);
    assert_ne!(original_head, repaired_head);
    let text = error.to_string();
    assert!(
        text.contains(&format!("head revision {repaired_head}")),
        "{text}"
    );
    for expected in [
        "Already published",
        "acme/project#17",
        &branch,
        "main",
        "Original delivery result: delivery failed with crash",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert!(!text.contains("test-token"), "{text}");
    assert!(!text.contains("Error {"), "{text}");
    assert!(!text.contains("Api("), "{text}");
    let withdrawn = authority.withdrawn_reviews.lock().unwrap();
    assert_eq!(withdrawn.last().unwrap().head_revision, original_head);
}

#[tokio::test]
async fn exhausted_delivery_loops_retain_latest_verifier_evidence() {
    use crate::v2_run_ledger::RunEvent;

    for repair in [false, true] {
        let repo = TempRepo::delivery();
        if repair {
            git(
                &repo.workspace,
                &[
                    "config",
                    "filter.failure.clean",
                    "printf 'repair test-token\\n' >&2; exit 1",
                ],
            );
            git(
                &repo.workspace,
                &["config", "filter.failure.required", "true"],
            );
            fs::write(
                repo.workspace.join(".gitattributes"),
                "*.guard filter=failure\n",
            )
            .assert_value();
            fs::write(repo.workspace.join("candidate.guard"), "candidate work\n").assert_value();
        }
        let mode = DeliveryMode::Merge;
        let runtime = admitted(&repo, mode).await.runtime;
        let graph = full_graph(vec![
            json!({
                "kind":"loop","name":"publication","state":{"kind":"null"},
                "maxIterations":2,"promotedStatePaths":[],"body":delivery_node(mode),
                "until":{"kind":"in","value":{"name":"deliver","source":"signal","field":"delivery"},"labels":["merged"]}
            }),
            json!({"kind":"fail","name":"halt","reason":"publication_incomplete"}),
        ]);
        let admitted = NativeV2Admission
            .admit(RunSubmission {
                environment: None,
                title: RunTitle::new("Delivery evidence").assert_value(),
                graph,
                initial_input: Value::Null,
                runtime,
                source: source_for_revision(&repo.base),
                submission_key: IdempotencyKey::new("delivery-evidence").assert_value(),
            })
            .await
            .assert_value();
        let adapter = retained_adapter(
            &repo,
            Arc::new(FakeGitHub::new(repo.remote.clone(), Script::CiFailed)),
            DeliveryPollPolicy::new(2, Duration::ZERO).assert_value(),
            DeliveryLineage::original("delivery-evidence"),
        );
        let runner =
            Arc::new(NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value());
        let (run_id, ledger, environments) = routing::create_delivery_run(admitted).await;
        let terminal =
            NativeV2Supervisor::new(run_id.clone(), ledger.clone(), runner, environments)
                .drive()
                .await
                .assert_value();
        let TerminalResult::Failed {
            context: Some(context),
            ..
        } = &terminal
        else {
            panic!("missing delivery evidence")
        };
        assert!(context.primary.is_none());
        assert_eq!(context.exhausted_loops.len(), 1);
        assert_eq!(context.exhausted_loops[0].node.as_str(), "publication");
        assert_eq!(context.exhausted_loops[0].completed_iterations.get(), 2);
        assert_eq!(context.verifiers.len(), 1);
        let latest = &context.verifiers[0];
        let label = if repair {
            DELIVERY_REPAIR_REQUIRED_LABEL
        } else {
            DELIVERY_CI_FAILED_LABEL
        };
        assert!(latest.message.as_str().contains(label));
        assert!(!latest.message.as_str().contains("test-token"));
        if repair {
            assert!(latest.message.as_str().contains("[REDACTED]"));
        }
        let stored = ledger.get(&run_id).await.assert_value().assert_value();
        assert_eq!(stored.snapshot.terminal.as_ref(), Some(&terminal));
        let tail = ledger.snapshot_and_tail(&run_id, None).await.assert_value();
        let evidence = tail
            .events
            .iter()
            .filter(|entry| {
                matches!(
                    entry.event,
                    RunEvent::SafeLog {
                        diagnostic: Some(FailureDiagnosticKind::Verifier),
                        ..
                    }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(evidence.len(), 2);
        let last = evidence.last().assert_value();
        assert_eq!(Some(&last.cursor), latest.cursor.as_ref());
        let RunEvent::SafeLog {
            execution, line, ..
        } = &last.event
        else {
            panic!("missing verifier log")
        };
        assert_eq!(line.as_str(), latest.message.as_str());
        let execution = stored
            .snapshot
            .executions
            .get(execution.as_ref().assert_value())
            .assert_value();
        assert_eq!(
            latest.execution.as_ref().assert_value().execution,
            execution.reference.public_execution().assert_value()
        );
        assert!(
            matches!(&tail.events.last().assert_value().event, RunEvent::Terminal { result } if result == &terminal)
        );
    }
}

#[tokio::test]
async fn terminal_delivery_diagnostics_keep_real_causes_and_typed_outcomes() {
    for (script, expected, detail) in [
        (
            Script::PolicyForbidden,
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
            "HTTP 403",
        ),
        (
            Script::PolicySchemaInvalid,
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
            "missing field `repository`",
        ),
        (
            Script::InspectFailed,
            WorkerOutcome::declared_failure(WorkerErrorCode::Timeout),
            "unfamiliar error",
        ),
        (
            Script::HeadUpdatePermanentFailure,
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
            "branch update rejected",
        ),
        (
            Script::CredentialExpires,
            WorkerOutcome::authentication_refusal(),
            "Bad credentials",
        ),
        (
            Script::PendingReadiness,
            WorkerOutcome::declared_failure(WorkerErrorCode::Timeout),
            "confirmation timed out",
        ),
        (
            Script::InvalidReviewObservation,
            WorkerOutcome::malformed(),
            "review observation",
        ),
    ] {
        let (repo, authority) = delivery_harness(script);
        let execution = run_delivery_execution(
            DeliveryRunRequest {
                repo: &repo,
                attempts: 2,
                mode: DeliveryMode::Merge,
                run_id: "delivery-diagnostic",
                refresh: None,
            },
            authority,
        )
        .await;
        assert_eq!(
            execution.outcome, expected,
            "outcome changed for {script:?}"
        );
        let failures = execution
            .output
            .iter()
            .filter(|output| output.diagnostic == Some(FailureDiagnosticKind::Failure))
            .collect::<Vec<_>>();
        assert_eq!(
            failures.len(),
            1,
            "one terminal diagnostic for {script:?}: {:?}",
            execution.output
        );
        assert_eq!(failures[0].stream, LiveOutputStream::Error);
        assert!(
            failures[0].text.contains(detail),
            "missing cause for {script:?}: {:?}",
            failures[0]
        );
        assert!(!failures[0].text.contains("test-token"));
        assert!(
            !failures[0]
                .text
                .contains(&git_auth::encode_basic_credential("test-token"))
        );
    }
}

#[tokio::test]
async fn missing_delivery_credential_has_a_cause_without_changing_authentication_refusal() {
    let (repo, authority) = delivery_harness(Script::NoCi);
    let adapter = retained_adapter(
        &repo,
        authority,
        DeliveryPollPolicy::new(1, Duration::ZERO).assert_value(),
        DeliveryLineage::original("missing-credential"),
    );
    let execution = run_with_adapter_token(
        DeliveryRunRequest {
            repo: &repo,
            attempts: 1,
            mode: DeliveryMode::Merge,
            run_id: "missing-credential",
            refresh: None,
        },
        adapter,
        PullRequestFeedback::Consider,
        None,
    )
    .await;
    assert_eq!(execution.outcome, WorkerOutcome::authentication_refusal());
    assert!(execution.output.iter().any(|output| output.diagnostic
        == Some(FailureDiagnosticKind::Failure)
        && output.text.contains("GH_TOKEN")
        && output.text.contains("not supplied")));
}
