use super::*;

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
