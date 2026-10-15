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
