use super::*;

#[test]
fn pending_ci_never_becomes_pull_request_ready() {
    assert!(matches!(
        ReviewProgress::from_open(SourceChecks::Pending, true, false),
        ReviewProgress::Pending
    ));
}

#[test]
fn passed_ci_can_be_ready_while_approval_is_missing() {
    assert!(matches!(
        ReviewProgress::from_open(SourceChecks::Passed, true, false),
        ReviewProgress::PullRequestReady
    ));
}

#[test]
fn passed_ci_without_the_approval_exception_is_mergeable() {
    assert!(matches!(
        ReviewProgress::from_open(SourceChecks::Passed, false, false),
        ReviewProgress::Mergeable
    ));
}

#[tokio::test]
async fn physical_cleanup_failure_cannot_be_overridden_by_a_merge_or_withdrawal_result() {
    use crate::native_v2_delivery::tests::github_fixture::{FakeGitHub, Script};
    for (script, original) in [
        (Script::QueuedMergeRace, NodeRunnerError::CleanupUnconfirmed),
        (
            Script::QueuedWithdrawalCleanup,
            NodeRunnerError::DriverDetail("host refused".to_owned()),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let authority = Arc::new(FakeGitHub::new(root.path().to_owned(), script));
        let review = SourceReviewReceipt {
            review_id: "17".to_owned(),
            repository: "acme/project".to_owned(),
            target_branch: "main".to_owned(),
            head_branch: "zeroshot/cleanup".to_owned(),
            head_revision: "b".repeat(40),
        };
        authority
            .request_merge(&review, SourceDeliveryCredential::token("test-token"))
            .await
            .unwrap();
        let adapter = NativeV2DeliveryAdapter::new(
            NativeV2DeliveryConfig {
                delivery_run_id: openengine_cluster_protocol::RunId::new("cleanup"),
                adopt_existing_delivery: false,
                git_identity: None,
                workspace: root.path().to_owned(),
                git_program: "/usr/bin/git".into(),
                target: DeliveryTarget::new("acme/project", "main", "a".repeat(40)).unwrap(),
                poll: DeliveryPollPolicy::until_cancelled(Duration::from_millis(1)),
            },
            authority,
        );
        adapter.record_published(review);
        let mut credentials = DeliveryCredentials {
            environment: None,
            token_file: None,
            token: "test-token".to_owned(),
            git: None,
            forge_field: Some(GITHUB_TOKEN_ENV),
            github_git_token: "test-token".to_owned(),
            token_from_environment: false,
        };
        let response = NodeResponseContract::Verifier {
            output: delivery_result_schema(DeliveryMode::MergeV3).unwrap(),
            signals: BTreeMap::from([(
                FieldName::new(DELIVERY_SIGNAL_FIELD).unwrap(),
                delivery_signal_labels(DeliveryMode::MergeV3).unwrap(),
            )]),
            diagnostic: delivery_diagnostic_schema().unwrap(),
        };
        assert!(matches!(
            adapter
                .finish_merge_handoff(
                    (DeliveryMode::MergeV3, &response),
                    &mut credentials,
                    Err(original)
                )
                .await,
            Err(NodeRunnerError::CleanupUnconfirmed)
        ));
    }
}
