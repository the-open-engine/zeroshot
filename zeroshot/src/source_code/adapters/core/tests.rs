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
