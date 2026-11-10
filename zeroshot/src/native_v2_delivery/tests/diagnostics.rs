use super::*;

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
