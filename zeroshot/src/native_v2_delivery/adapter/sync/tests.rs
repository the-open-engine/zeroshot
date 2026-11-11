use super::*;
use crate::native_v2_runner::test_support;
use crate::source_code_provider::adapters::core::gitlab::tests::Fixture;
use openengine_cluster_testkit::assertions::AssertValue;

#[tokio::test]
async fn gitlab_review_changes_after_preflight_redact_every_synchronization_record() {
    let branch = "zeroshot/test";
    let head = "b".repeat(40);
    for (field, value, reason) in [
        ("state", "closed", "was closed"),
        ("target_branch", "release", "changed the bound review/head"),
    ] {
        let fixture = Fixture::new().await;
        fixture.open_review();
        let (adapter, request, mut credentials) = gitlab_sync(&fixture);
        let snapshot = adapter
            .authority
            .observe_delivery(
                SourceDeliveryRead {
                    target: &request.target,
                    head_branch: branch,
                    known_review: None,
                    include_review: true,
                },
                credentials.current(),
            )
            .await
            .assert_value();
        let review = snapshot.review.assert_value();
        assert!(matches!(review.state, SourceReviewState::Open { .. }));
        adapter.record_published(SourceReviewReceipt::for_request(&request, "17"));

        // The successful preflight must not hide a later human change from synchronization.
        fixture.change_review(field, serde_json::json!(value));
        let (_cancel, control, mut output) = test_support::control_with_capacity(8);
        let stop = match adapter
            .synchronize_review(&request, &mut credentials, &control)
            .await
        {
            Ok(_) => panic!("changed review must refuse synchronization"),
            Err(stop) => stop,
        };
        assert!(stop.diagnostic().assert_value().contains(reason));
        adapter
            .report_stop(
                &stop,
                DeliveryDiagnostic {
                    credentials: &credentials,
                    control: &control,
                    repair_supported: false,
                },
            )
            .await;
        assert_eq!(
            stop.result().assert_value(),
            WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)
        );
        drop(control);
        let mut records = Vec::new();
        while let Ok(record) = output.recv_output().await {
            for secret in [branch, &head, "test-private-token"] {
                assert!(!record.text.contains(secret), "{}", record.text);
            }
            assert!(record.text.contains("[REDACTED]"), "{}", record.text);
            assert!(record.text.contains(reason), "{}", record.text);
            assert!(
                record.text.contains("group/sub/project!17"),
                "{}",
                record.text
            );
            records.push(record);
        }
        assert_eq!(records.len(), 2, "synchronization and terminal diagnostics");
        assert!(records[0].text.contains("failed after 1 attempt(s)"));
        assert!(records[1].text.contains("Already published"));
    }
}

#[tokio::test]
async fn gitlab_identity_errors_redact_credentials_before_bounds_and_context_wrapping() {
    use crate::source_code_provider::MAX_SOURCE_API_DIAGNOSTIC_BYTES;

    let fixture = Fixture::new().await;
    fixture.open_review();
    let (adapter, request, mut credentials) = gitlab_sync(&fixture);
    let marker = "identity-payload-";
    let token = "test-private-token";
    let secret_prefix = "test-private-";
    fixture.change_review(
        "target_branch",
        serde_json::json!(format!("{marker}{token}")),
    );
    let error = adapter
        .authority
        .open_or_update_review(&request, credentials.current())
        .await
        .unwrap_err();
    let short = error.to_string();
    let SourceDeliveryError::Identity(detail) = error else {
        panic!("invalid review identity must remain a refusal");
    };
    let prefix_bytes = detail.find(marker).assert_value();
    let cutoff = MAX_SOURCE_API_DIAGNOSTIC_BYTES - "\n[diagnostic truncated]".len();
    let padding = "x".repeat(cutoff - prefix_bytes - marker.len() - secret_prefix.len());
    fixture.change_review(
        "target_branch",
        serde_json::json!(format!("{marker}{padding}{token} trailing response text")),
    );

    let error = adapter
        .authority
        .open_or_update_review(&request, credentials.current())
        .await
        .unwrap_err();
    assert!(matches!(error, SourceDeliveryError::Identity(_)));
    assert!(!error.authentication_failed());
    assert!(!error.retryable_operation());
    assert!(!error.retryable_review_sync());
    let direct = error.to_string();
    assert!(
        !direct.contains(secret_prefix),
        "echoed credential survived the cutoff"
    );
    assert!(!short.contains(token));
    assert!(direct.contains("<invalid branch>"));

    let (_cancel, control, mut output) = test_support::control_with_capacity(16);
    let stop = match adapter
        .synchronize_review(&request, &mut credentials, &control)
        .await
    {
        Ok(_) => panic!("invalid review identity must refuse synchronization"),
        Err(stop) => stop,
    };
    adapter
        .report_stop(
            &stop,
            DeliveryDiagnostic {
                credentials: &credentials,
                control: &control,
                repair_supported: false,
            },
        )
        .await;
    assert!(!stop.diagnostic().assert_value().contains(secret_prefix));
    assert_eq!(
        stop.result().assert_value(),
        WorkerOutcome::declared_failure(WorkerErrorCode::Refusal)
    );
    drop(control);
    let mut records = Vec::new();
    while let Ok(record) = output.recv_output().await {
        assert!(!record.text.contains(secret_prefix));
        assert!(record.text.len() <= crate::native_v2_runner::MAX_LIVE_OUTPUT_BYTES);
        records.push(record);
    }
    assert!(
        records.len() >= 2,
        "synchronization and terminal diagnostics"
    );
    let terminal = records.last().assert_value();
    assert_eq!(terminal.diagnostic, Some(FailureDiagnosticKind::Failure));
}

fn gitlab_sync(
    fixture: &Fixture,
) -> (
    NativeV2DeliveryAdapter,
    SourceReviewRequest,
    DeliveryCredentials<'static>,
) {
    let request = SourceReviewRequest {
        target: DeliveryTarget::new("group/sub/project", "main", "a".repeat(40)).assert_value(),
        head_branch: "zeroshot/test".to_owned(),
        head_revision: "b".repeat(40),
        title: "Delivery".to_owned(),
        description: "Preserve behavior.".to_owned(),
        source_issue: None,
    };
    let adapter = NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            delivery_run_id: openengine_cluster_protocol::RunId::new("sync-redaction"),
            adopt_existing_delivery: false,
            git_identity: None,
            commit_identity: None,
            commit_co_author: None,
            workspace: std::env::temp_dir(),
            git_program: PathBuf::from("unused-git"),
            target: request.target.clone(),
            poll: DeliveryPollPolicy::new(1, Duration::ZERO).assert_value(),
        },
        Arc::new(fixture.authority.clone()),
    )
    .with_trusted_github_token(Some(Arc::from(request.head_branch.as_str())));
    let credentials = DeliveryCredentials {
        environment: None,
        token_file: None,
        token: "test-private-token".to_owned(),
        git: None,
        forge_field: Some("GITLAB_TOKEN"),
        github_git_token: request.head_revision.clone(),
        token_from_environment: false,
    };
    (adapter, request, credentials)
}

#[tokio::test]
async fn credential_refresh_respects_remaining_deadline() {
    let (_sender, receiver) = tokio::sync::watch::channel(false);
    let result = refresh_within_deadline(
        std::future::pending::<Result<(), EnvironmentRefreshError>>(),
        DriverCancellation::new(receiver),
        Duration::from_millis(1),
    )
    .await;

    assert!(matches!(result, Ok(CredentialRefreshProgress::TimedOut)));
}

#[tokio::test]
async fn credential_refresh_respects_cancellation() {
    let (sender, receiver) = tokio::sync::watch::channel(false);
    assert!(sender.send(true).is_ok());
    let result = refresh_within_deadline(
        std::future::pending::<Result<(), EnvironmentRefreshError>>(),
        DriverCancellation::new(receiver),
        Duration::from_secs(1),
    )
    .await;

    assert!(matches!(
        result,
        Err(DeliveryStop::Runner(NodeRunnerError::Cancelled))
    ));
}
