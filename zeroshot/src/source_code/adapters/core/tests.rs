use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

#[test]
fn github_delivery_requires_a_matching_remote_identity() {
    for (url, repository, valid) in [
        ("https://github.com/acme/project.git", "acme/project", true),
        ("git@github.com:acme/project.git", "acme/project", true),
        (
            "ssh://git@github.com/acme/project.git",
            "ACME/Project",
            true,
        ),
        (
            "ssh://git@ssh.github.com:443/acme/project.git",
            "acme/project",
            true,
        ),
        (
            "https://ssh.github.com/acme/project.git",
            "acme/project",
            false,
        ),
        (
            "ssh://git@ssh.github.com:22/acme/project.git",
            "acme/project",
            false,
        ),
        (
            "ssh://git@ssh.github.com/acme/project.git",
            "acme/project",
            false,
        ),
        ("git@ssh.github.com:acme/project.git", "acme/project", false),
        ("https://gitlab.com/acme/project.git", "acme/project", false),
        ("git@github.com:other/project.git", "acme/project", false),
    ] {
        let source: ResolvedSource = serde_json::from_value(json!({
            "repository": "caller/opaque/source-label", "branch": "main",
            "revision": "a".repeat(40),
            "remote": {"url": url, "forge": {
                "provider": GITHUB_PROVIDER_ID, "repository": repository
            }}
        }))
        .assert_value();
        let detected = forge_identity(&source.remote.as_ref().assert_value().url).assert_value();
        assert_eq!(
            detected
                .is_some_and(|forge| forge.repository.as_str().eq_ignore_ascii_case(repository)),
            valid,
            "remote identification: {url} / {repository}"
        );
        for capability in [SourceCapability::PullRequest, SourceCapability::Merge] {
            assert_eq!(
                validate_delivery_source(&source, &[capability]).is_ok(),
                valid,
                "{url} / {repository} / {capability:?}"
            );
            assert_eq!(
                validate_delivery_repository(
                    source.repository.as_str(),
                    source.remote.as_ref(),
                    &[capability]
                )
                .is_ok(),
                valid,
                "repository preflight: {url} / {repository} / {capability:?}"
            );
        }
        validate_delivery_source(&source, &[SourceCapability::Read, SourceCapability::Push])
            .assert_value();
    }
    let legacy: ResolvedSource = serde_json::from_value(json!({
        "repository": "acme/project", "branch": "main", "revision": "a".repeat(40)
    }))
    .assert_value();
    validate_delivery_source(
        &legacy,
        &[SourceCapability::PullRequest, SourceCapability::Merge],
    )
    .assert_value();
}

#[cfg(unix)]
#[tokio::test]
async fn delivery_factory_keeps_private_push_diagnostics_for_git_and_github() {
    let root = tempfile::tempdir().assert_value();
    let program = root.path().join("git-push-failure");
    let secret = "factory-private-git-token";
    openengine_cluster_testkit::fixture::write_executable(
        &program,
        "#!/bin/sh\nprintf 'factory-private-git-token\\n'\nprintf 'fixture push rejected\\n' >&2\nexit 1\n",
        0o700,
    )
    .assert_value();
    for (url, capability) in [
        (
            "https://forge.example/team/project.git",
            SourceCapability::Push,
        ),
        (
            "https://github.com/owner/project.git",
            SourceCapability::PullRequest,
        ),
    ] {
        let github = capability == SourceCapability::PullRequest;
        let source: ResolvedSource = serde_json::from_value(json!({
            "repository":"owner/project", "branch":"main", "revision":"a".repeat(40),
            "remote":{"url":url,"forge": github.then(|| json!({
                "provider":GITHUB_PROVIDER_ID,"repository":"owner/project"
            }))}
        }))
        .assert_value();
        let run_id = openengine_cluster_protocol::RunId::new("source-factory-diagnostic");
        let diagnostics = Arc::new(OperatorDiagnosticStore::default());
        let authority = delivery_authority(SourceDeliveryFactoryRequest {
            source: &source,
            workspace: root.path(),
            git_program: &program,
            gh_program: &root.path().join("forge-cli-must-not-run"),
            home_directory: root.path(),
            private_directory: root.path(),
            git_identity: None,
            local_environment: None,
            required_capabilities: &[capability],
            operator_diagnostics: Some((run_id.clone(), diagnostics.clone())),
        })
        .assert_value();
        assert_eq!(
            authority.forge_credential_field(),
            github.then_some("GH_TOKEN")
        );
        let request = SourcePushRequest {
            workspace: root.path().to_owned(),
            target: DeliveryTarget::new("owner/project", "main", "a".repeat(40)).assert_value(),
            head_branch: "zeroshot/diagnostic".to_owned(),
            head_revision: "b".repeat(40),
        };

        let error = authority
            .push_branch(&request, SourceDeliveryCredential::token(secret))
            .await
            .expect_err("failed Git publication must not produce a successful receipt");

        assert!(!error.to_string().contains(secret));
        let snapshot = diagnostics.snapshot(&run_id);
        assert_eq!(snapshot.diagnostics.len(), 1);
        let diagnostic = &snapshot.diagnostics[0];
        assert_eq!(diagnostic.run_id, run_id);
        assert_eq!(diagnostic.code, "git_push_failed");
        assert_eq!(diagnostic.operation, "delivery.git_push");
        assert_eq!(diagnostic.exit_status, Some(1));
        assert!(diagnostic.stdout.contains("[REDACTED]"));
        assert!(diagnostic.stderr.contains("fixture push rejected"));
        assert!(!diagnostic.stdout.contains(secret));
        assert!(
            diagnostics
                .snapshot(&openengine_cluster_protocol::RunId::new("other-run"))
                .diagnostics
                .is_empty()
        );
    }
}

struct RecordingCheckout {
    seen: std::sync::Mutex<Option<(String, String, String)>>,
}

impl RecordingCheckout {
    fn new() -> Self {
        Self {
            seen: std::sync::Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl SourceMaterializationTarget for RecordingCheckout {
    async fn checkout_git(
        &self,
        request: SourceGitCheckout<'_>,
    ) -> Result<SourceContentDigest, SourceMaterializationError> {
        *self.seen.lock().unwrap() = Some((
            request.remote.to_owned(),
            request.branch.to_owned(),
            request.revision.to_string(),
        ));
        SourceContentDigest::new("d".repeat(64)).map_err(|_| SourceMaterializationError)
    }

    fn is_available(&self) -> bool {
        true
    }

    fn write_file(&self, _name: &str, _contents: &[u8]) -> Result<(), SourceMaterializationError> {
        Err(SourceMaterializationError)
    }
}

fn core_source(remote: Option<(&str, &str)>) -> ResolvedSource {
    let mut wire = json!({
        "repository": "acme/project",
        "branch": "main",
        "revision": "a".repeat(40),
    });
    if let Some((url, repository)) = remote {
        wire["remote"] = json!({
            "url": url,
            "forge": {"provider": GITHUB_PROVIDER_ID, "repository": repository}
        });
    }
    serde_json::from_value(wire).assert_value()
}

fn identify_request(provider: &CoreSourceProvider) -> SourceIdentifyRepositoryRequest {
    SourceIdentifyRepositoryRequest::new(
        provider.descriptor().provider().clone(),
        SourceProfileId::new("default").assert_value(),
        (
            SourceAccountId::new("workspace").assert_value(),
            SourceCredentialHandleId::new("git-transport").assert_value(),
        ),
        SourceRepositoryReference::new("acme/project").assert_value(),
    )
    .assert_value()
}

fn operation_request(repository: CanonicalRepository) -> SourceOperationRequest {
    SourceOperationRequest::new(
        repository,
        SourceCredentialHandleId::new("git-transport").assert_value(),
        (
            SourceWorkspaceId::new("b".repeat(64)).assert_value(),
            SourceOperationId::new("branch-operation").assert_value(),
        ),
        SourceOperation::Branch {
            expected_parent: SourceRevisionId::new("a".repeat(40)).assert_value(),
            branch: SourceBranchId::new("feature").assert_value(),
            pre_effect: SourceStateDigest::new("c".repeat(64)).assert_value(),
        },
    )
    .assert_value()
}

#[tokio::test]
async fn maintained_git_provider_keeps_transport_only_capabilities() {
    let source = core_source(None);
    let provider = CoreSourceProvider::new(ProviderKind::Git, Some(source), None).assert_value();
    assert_eq!(
        provider.descriptor().provider().id().as_str(),
        GIT_PROVIDER_ID
    );
    assert!(
        provider
            .comment_destination_from_url("https://github.com/acme/project/issues/1")
            .is_none()
    );
    assert!(
        provider
            .repository_from_remote("https://github.com/acme/project.git")
            .is_none()
    );
    assert!(provider.comment_authority().is_none());
    assert!(provider.delivery_authority().is_none());

    let identify = identify_request(&provider);
    let repository = provider.identify_repository(&identify).await.assert_value();
    assert_eq!(repository.provider(), provider.descriptor().provider());
    assert_eq!(repository.profile().as_str(), "default");
    assert_eq!(repository.account().as_str(), "workspace");
    assert_eq!(repository.repository().as_str(), "acme/project");

    let inspect = SourceInspectRepositoryRequest::new(
        repository.clone(),
        identify.credential_handle().clone(),
    )
    .assert_value();
    let inspection = provider.inspect_repository(&inspect).await.assert_value();
    assert_eq!(inspection.repository(), &repository);
    assert_eq!(inspection.default_revision().as_str(), "a".repeat(40));
    assert!(inspection.public_urls().is_empty());
    assert!(
        provider
            .inspect_operation(&operation_request(repository))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn maintained_github_provider_uses_the_caller_forge_identity_and_checkout() {
    let source = core_source(Some((
        "https://github.com/acme/project.git",
        "acme/project",
    )));
    let provider =
        CoreSourceProvider::new(ProviderKind::GitHub, Some(source.clone()), None).assert_value();
    assert_eq!(
        provider.descriptor().provider().id().as_str(),
        GITHUB_PROVIDER_ID
    );

    let issue = provider
        .comment_destination_from_url("https://github.com/acme/project/issues/17")
        .assert_value();
    assert_eq!(issue.repository, "acme/project");
    assert_eq!(issue.discussion_id, "17");
    assert_eq!(issue.kind, SourceDiscussionKind::Issue);
    assert!(
        provider
            .comment_destination_from_url("https://gitlab.com/acme/project/issues/17")
            .is_none()
    );
    assert_eq!(
        provider
            .repository_from_remote("https://github.com/acme/project.git")
            .assert_value()
            .as_str(),
        "acme/project"
    );
    assert!(
        provider
            .repository_from_remote("git@gitlab.com:acme/project.git")
            .is_none()
    );

    let identify = identify_request(&provider);
    let repository = provider.identify_repository(&identify).await.assert_value();
    assert_eq!(
        repository.repository().as_str(),
        source
            .remote
            .as_ref()
            .and_then(|remote| remote.forge.as_ref())
            .assert_value()
            .repository
            .as_str()
    );

    let materialize = SourceMaterializeRequest::new(
        repository.clone(),
        identify.credential_handle().clone(),
        SourceRevisionId::new(source.revision.as_str()).assert_value(),
    )
    .assert_value();
    let target = RecordingCheckout::new();
    let destination = SourceMaterializationDestination::new(&target);
    let receipt = provider
        .materialize(&materialize, destination)
        .await
        .assert_value();
    assert_eq!(receipt.repository(), &repository);
    assert_eq!(receipt.revision().as_str(), source.revision.as_str());
    assert_eq!(receipt.content_digest().as_str(), "d".repeat(64));
    assert_eq!(
        target.seen.lock().unwrap().as_ref().assert_value(),
        &(
            "https://github.com/acme/project.git".to_owned(),
            "main".to_owned(),
            source.revision.as_str().to_owned(),
        )
    );
}

#[tokio::test]
async fn source_less_core_provider_refuses_every_source_operation() {
    for kind in [ProviderKind::Git, ProviderKind::GitHub] {
        let provider = CoreSourceProvider::new(kind, None, None).assert_value();
        let identify = identify_request(&provider);
        assert!(provider.identify_repository(&identify).await.is_err());

        let repository = CanonicalRepository::new(
            identify.provider().clone(),
            identify.profile().clone(),
            identify.account().clone(),
            SourceRepositoryId::new("acme/project").assert_value(),
        )
        .assert_value();
        let inspect = SourceInspectRepositoryRequest::new(
            repository.clone(),
            identify.credential_handle().clone(),
        )
        .assert_value();
        assert!(provider.inspect_repository(&inspect).await.is_err());

        let materialize = SourceMaterializeRequest::new(
            repository.clone(),
            identify.credential_handle().clone(),
            SourceRevisionId::new("a".repeat(40)).assert_value(),
        )
        .assert_value();
        let harness = unsafe { SourceMaterializationContractHarness::new() };
        assert!(
            provider
                .materialize(&materialize, harness.destination())
                .await
                .is_err()
        );
        assert_eq!(harness.write_count(), 0);

        let operation = operation_request(repository);
        assert!(provider.inspect_operation(&operation).await.is_err());
        let mut handle = ();
        let workspace = unsafe {
            SourceWorkspaceCapability::from_verified_contract_test(
                operation.workspace().clone(),
                &mut handle,
            )
        };
        assert!(provider.operate(&operation, workspace).await.is_err());
    }
}
