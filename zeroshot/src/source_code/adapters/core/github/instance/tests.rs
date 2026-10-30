use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

fn authority(root: &std::path::Path, version: &str) -> GhCliDeliveryAuthority {
    let program = root.join("gh-instance-fixture");
    let script = format!(
        r##"#!/bin/sh
test "$GH_HOST" = "ghe.example" || exit 90
test "${{GH_ENTERPRISE_TOKEN:-}}" = "instance-test-token" || exit 91
test -z "${{GH_TOKEN:-}}" || exit 92
printf '%s\n' "$2" >> "$HOME/calls"
case "$2" in
  meta) printf '%s' '{{"installed_version":"{version}"}}' ;;
  user) printf '%s' '{{"id":42}}' ;;
  *) exit 93 ;;
esac
"##
    );
    openengine_cluster_testkit::fixture::write_executable(&program, script, 0o700).assert_value();
    GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        gh_program: program,
        ..GhCliAuthorityConfig::hosted(root.to_owned())
    })
    .with_base_url(SourceForgeBaseUrl::new("https://ghe.example").assert_value())
    .assert_value()
}

#[tokio::test]
async fn ghe_routes_to_the_selected_host_and_verifies_its_version_once() {
    let root = tempfile::tempdir().assert_value();
    let authority = authority(root.path(), "3.22.2");
    for _ in 0..2 {
        let result = authority
            .api(
                &["user".to_owned()],
                SourceDeliveryCredential::token("instance-test-token"),
            )
            .await
            .assert_value();
        assert_eq!(result["id"], 42);
    }
    assert_eq!(
        std::fs::read_to_string(root.path().join("calls")).assert_value(),
        "meta\nuser\nuser\n"
    );
}

#[tokio::test]
async fn older_or_unknown_ghe_versions_refuse_before_the_requested_operation() {
    for version in ["3.22.1", "3.21.9", "unknown"] {
        let root = tempfile::tempdir().assert_value();
        let error = authority(root.path(), version)
            .api(
                &["user".to_owned()],
                SourceDeliveryCredential::token("instance-test-token"),
            )
            .await
            .expect_err("unsupported server must refuse");
        assert!(error.to_string().contains("3.22.2"));
        assert!(!error.to_string().contains("instance-test-token"));
        assert_eq!(
            std::fs::read_to_string(root.path().join("calls")).assert_value(),
            "meta\n"
        );
    }
}

#[test]
fn cloud_and_enterprise_tokens_use_the_correct_cli_alias() {
    for (base, field) in [
        ("https://github.com", "GH_TOKEN"),
        ("https://acme.ghe.com", "GH_TOKEN"),
        ("https://ghe.example:8443", "GH_ENTERPRISE_TOKEN"),
    ] {
        let authority =
            GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted("/fixture".into()))
                .with_base_url(SourceForgeBaseUrl::new(base).assert_value())
                .assert_value();
        let command = authority.api_command(SourceDeliveryCredential::token("instance-test-token"));
        let environment: BTreeMap<_, _> = command
            .as_std()
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(environment[field].as_deref(), Some("instance-test-token"));
        let other = if field == "GH_TOKEN" {
            "GH_ENTERPRISE_TOKEN"
        } else {
            "GH_TOKEN"
        };
        assert!(environment.get(other).is_none_or(Option::is_none));
        assert_eq!(
            environment["GH_HOST"].as_deref(),
            Some(base.trim_start_matches("https://"))
        );
    }
}

#[test]
fn ghe_receipt_links_preserve_the_admitted_instance() {
    let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted("/fixture".into()))
        .with_base_url(SourceForgeBaseUrl::new("https://ghe.example:8443").assert_value())
        .assert_value();
    let receipt = SourceReviewReceipt {
        review_id: "17".to_owned(),
        repository: "acme/project".to_owned(),
        target_branch: "main".to_owned(),
        head_branch: "zeroshot/v2-run".to_owned(),
        head_revision: "b".repeat(40),
    };
    let links = authority.canonical_review_links(&receipt).assert_value();
    assert_eq!(links.url, "https://ghe.example:8443/acme/project/pull/17");
    assert_eq!(
        links.commit_url,
        format!(
            "https://ghe.example:8443/acme/project/commit/{}",
            receipt.head_revision
        )
    );
    assert!(
        authority
            .with_base_url(SourceForgeBaseUrl::new("https://ghe.example/path").assert_value())
            .is_err()
    );
}
