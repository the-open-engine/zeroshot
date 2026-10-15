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

#[cfg(unix)]
mod queued_handoff {
    use super::*;
    use crate::native_v2_delivery::tests::github_fixture::{FakeGitHub, Script};
    use std::sync::atomic::Ordering;

    struct Fixture {
        _root: tempfile::TempDir,
        authority: Arc<FakeGitHub>,
        adapter: NativeV2DeliveryAdapter,
        review: SourceReviewReceipt,
        response: NodeResponseContract,
    }

    impl Fixture {
        async fn new(script: Script) -> Self {
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
                authority.clone(),
            );
            adapter.record_published(review.clone());
            let response = NodeResponseContract::Verifier {
                output: delivery_result_schema(DeliveryMode::MergeV3).unwrap(),
                signals: BTreeMap::from([(
                    FieldName::new(DELIVERY_SIGNAL_FIELD).unwrap(),
                    delivery_signal_labels(DeliveryMode::MergeV3).unwrap(),
                )]),
                diagnostic: delivery_diagnostic_schema().unwrap(),
            };
            Self {
                _root: root,
                authority,
                adapter,
                review,
                response,
            }
        }

        async fn finish(
            &self,
            credentials: &mut DeliveryCredentials<'_>,
            original: Result<WorkerOutcome, NodeRunnerError>,
        ) -> Result<WorkerOutcome, NodeRunnerError> {
            self.adapter
                .finish_merge_handoff(
                    (DeliveryMode::MergeV3, &self.response),
                    credentials,
                    original,
                )
                .await
        }
    }

    fn credentials(file: Option<&TrustedGitHubTokenFile>) -> DeliveryCredentials<'_> {
        DeliveryCredentials {
            environment: None,
            token_file: file,
            token: "test-token".to_owned(),
            git: None,
            forge_field: Some(GITHUB_TOKEN_ENV),
            github_git_token: "test-token".to_owned(),
            token_from_environment: false,
        }
    }

    #[tokio::test]
    async fn physical_cleanup_failure_cannot_be_overridden_by_a_merge_or_withdrawal_result() {
        for (script, original) in [
            (Script::QueuedMergeRace, NodeRunnerError::CleanupUnconfirmed),
            (
                Script::QueuedWithdrawalCleanup,
                NodeRunnerError::DriverDetail("host refused".to_owned()),
            ),
        ] {
            let fixture = Fixture::new(script).await;
            assert!(matches!(
                fixture.finish(&mut credentials(None), Err(original)).await,
                Err(NodeRunnerError::CleanupUnconfirmed)
            ));
        }
    }

    #[tokio::test]
    async fn withdrawal_refreshes_a_rotated_file_once_and_never_hands_off_a_live_queue() {
        use std::io::Write as _;
        for issued in [
            Some("refreshed-token"),
            Some("test-token"),
            Some("bad token"),
            None,
        ] {
            let fixture = Fixture::new(Script::QueuedWithdrawalCredentialExpires).await;
            let private = tempfile::tempdir().unwrap();
            let path = private.path().join("token");
            if let Some(token) = issued {
                let mut file = crate::execution::platform::private_file(
                    &path,
                    crate::execution::platform::FileAccess::CreateNew,
                )
                .unwrap();
                file.write_all(token.as_bytes()).unwrap();
            }
            let capability =
                TrustedGitHubTokenFile::new(path, &fixture.adapter.config.workspace).unwrap();
            let mut current = credentials(Some(&capability));
            let error = fixture
                .finish(
                    &mut current,
                    Err(NodeRunnerError::DriverDetail("host refused".to_owned())),
                )
                .await
                .unwrap_err();
            assert!(matches!(error, NodeRunnerError::DriverDetail(_)));
            let rotated = issued == Some("refreshed-token");
            assert_eq!(
                fixture.authority.queue_live.load(Ordering::SeqCst),
                !rotated
            );
            assert_eq!(
                fixture.authority.withdrawn_reviews.lock().unwrap().len(),
                if rotated { 2 } else { 1 }
            );
            if rotated {
                assert_eq!(current.token, "refreshed-token");
                assert!(
                    matches!(&error, NodeRunnerError::DriverDetail(detail) if detail == "host refused")
                );
            } else {
                assert!(
                    error
                        .to_string()
                        .contains("No repair handoff is authorized")
                );
                assert!(error.to_string().contains(&fixture.review.head_revision));
            }
            for secret in ["test-token", "refreshed-token", "bad token"] {
                assert!(!error.to_string().contains(secret));
            }
        }
    }

    #[tokio::test]
    async fn an_invalid_authoritative_merge_revision_never_authorizes_a_success_or_repair() {
        for original in [
            Err(NodeRunnerError::DriverDetail(
                "host refusal test-token".to_owned(),
            )),
            Ok(WorkerOutcome::declared_failure(WorkerErrorCode::Timeout)),
        ] {
            let fixture = Fixture::new(Script::QueuedInvalidMergeRevision).await;
            let error = fixture
                .finish(&mut credentials(None), original)
                .await
                .unwrap_err();
            assert!(matches!(error, NodeRunnerError::DriverDetail(_)));
            let detail = error.to_string();
            assert!(detail.contains("invalid authoritative merged revision"));
            assert!(detail.contains("acme/project#17"));
            assert!(detail.contains(&fixture.review.head_revision));
            assert!(!detail.contains("test-token"));
        }
    }
}
