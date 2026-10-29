use super::*;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

#[test]
fn ambient_credentials_default_only_to_the_public_provider_hosts() {
    let values = BTreeMap::from([
        ("GH_TOKEN".to_owned(), "public-token".to_owned()),
        (
            "GH_ENTERPRISE_TOKEN".to_owned(),
            "enterprise-token".to_owned(),
        ),
        ("GITLAB_TOKEN".to_owned(), "gitlab-token".to_owned()),
    ]);
    for (provider, host, expected) in [
        (GITHUB, "github.com", Some("public-token")),
        (GITHUB, "example.ghe.com", None),
        (GITHUB, "git.example.test", None),
        (GITLAB, "gitlab.com", Some("gitlab-token")),
        (GITLAB, "git.example.test", None),
    ] {
        assert_eq!(
            environment_token(provider, host, &values)
                .assert_value()
                .as_deref(),
            expected,
            "{provider} {host}"
        );
    }
}

#[test]
fn every_ambient_alias_requires_the_selected_host() {
    for (provider, host, selector, aliases) in [
        (
            GITHUB,
            "company.ghe.com",
            "GH_HOST",
            &["GH_TOKEN", "GITHUB_TOKEN"][..],
        ),
        (
            GITHUB,
            "git.example.test",
            "GH_HOST",
            &["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"][..],
        ),
        (
            GITLAB,
            "git.example.test",
            "GITLAB_HOST",
            &["GITLAB_TOKEN", "GITLAB_ACCESS_TOKEN", "OAUTH_TOKEN"][..],
        ),
    ] {
        for alias in aliases {
            let mut values = BTreeMap::from([
                (selector.to_owned(), host.to_owned()),
                ((*alias).to_owned(), "selected-token".to_owned()),
            ]);
            assert_eq!(
                environment_token(provider, host, &values)
                    .assert_value()
                    .as_deref(),
                Some("selected-token"),
                "{alias}"
            );
            // A token for another host is neither returned nor validated.
            values.insert((*alias).to_owned(), "not a valid token\0".to_owned());
            for other_host in ["other.example.test", "github.com", "gitlab.com"] {
                assert!(
                    environment_token(provider, other_host, &values)
                        .assert_value()
                        .is_none(),
                    "{alias} {other_host}"
                );
            }
        }
    }
}

#[test]
fn host_selectors_normalize_https_authority_without_widening_it() {
    for (selector, host, allowed) in [
        ("FORGE.EXAMPLE.TEST", "forge.example.test", true),
        (
            "HTTPS://FORGE.EXAMPLE.TEST:443/",
            "forge.example.test",
            true,
        ),
        (
            "https://forge.example.test/install/",
            "forge.example.test",
            true,
        ),
        ("forge.example.test:8443", "forge.example.test:8443", true),
        (
            "https://forge.example.test:8443/",
            "forge.example.test:8443",
            true,
        ),
        ("forge.example.test:8443", "forge.example.test", false),
        (
            "https://user@forge.example.test/",
            "forge.example.test",
            false,
        ),
        (
            "https://forge.example.test/?host=other",
            "forge.example.test",
            false,
        ),
        (
            "https://forge.example.test/#other",
            "forge.example.test",
            false,
        ),
        ("http://forge.example.test", "forge.example.test", false),
        ("ssh://forge.example.test", "forge.example.test", false),
        ("", "gitlab.com", false),
        (" https://forge.example.test", "forge.example.test", false),
        ("forge.example.test\n", "forge.example.test", false),
    ] {
        let values = BTreeMap::from([
            ("GITLAB_HOST".to_owned(), selector.to_owned()),
            ("GITLAB_TOKEN".to_owned(), "selected-token".to_owned()),
        ]);
        assert_eq!(
            environment_token(GITLAB, host, &values)
                .assert_value()
                .is_some(),
            allowed,
            "{selector:?} {host}"
        );
    }
}

#[test]
fn credential_helper_refuses_duplicate_passwords_and_changed_authority() {
    for response in [
        "password=first\npassword=second\n",
        "host=other.example\npassword=secret\n",
        "protocol=http\npassword=secret\n",
        "not a credential\n",
    ] {
        git_credential_token(response, "git.example").assert_error();
    }
    assert_eq!(
        git_credential_token("username=oauth2\npassword=private-token\n\n", "git.example")
            .assert_value(),
        "private-token",
    );
}
