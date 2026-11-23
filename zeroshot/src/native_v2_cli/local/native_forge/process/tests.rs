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
fn windows_mixed_case_selectors_and_aliases_preserve_host_authority_and_precedence() {
    for (provider, host, selector, primary, secondary, public_host) in [
        (
            GITHUB,
            "private.example",
            "gh_host",
            "gh_enterprise_token",
            "GITHUB_ENTERPRISE_TOKEN",
            "github.com",
        ),
        (
            GITHUB,
            "company.ghe.com",
            "gH_HoSt",
            "gh_token",
            "GITHUB_TOKEN",
            "github.com",
        ),
        (
            GITLAB,
            "private.example",
            "gitlab_host",
            "GITLAB_TOKEN",
            "oauth_token",
            "gitlab.com",
        ),
        (
            GITLAB,
            "private.example",
            "GiTlAb_HoSt",
            "gitlab_token",
            "GITLAB_ACCESS_TOKEN",
            "gitlab.com",
        ),
    ] {
        let mut values = BTreeMap::from([
            (selector.to_owned(), host.to_owned()),
            (primary.to_owned(), "preferred-token".to_owned()),
            (secondary.to_owned(), "secondary-token".to_owned()),
        ]);
        if provider == GITHUB && host == "private.example" {
            values.insert("GH_TOKEN".to_owned(), "unrelated-public-token".to_owned());
        }
        let environment = LocalHarnessEnvironment::for_windows(values);
        assert_eq!(
            environment_token_from_snapshot(provider, host, &environment)
                .assert_value()
                .as_deref(),
            Some("preferred-token"),
            "{selector} {primary}"
        );
        for other_host in [public_host, "other.example"] {
            assert!(
                environment_token_from_snapshot(provider, other_host, &environment)
                    .assert_value()
                    .is_none(),
                "{selector} must not authorize {other_host}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn unix_selector_and_alias_names_remain_case_sensitive() {
    let values = BTreeMap::from([
        ("gitlab_host".to_owned(), "private.example".to_owned()),
        ("GITLAB_TOKEN".to_owned(), "canonical-token".to_owned()),
    ]);
    assert_eq!(
        environment_token(GITLAB, "gitlab.com", &values)
            .assert_value()
            .as_deref(),
        Some("canonical-token")
    );
    assert!(
        environment_token(GITLAB, "private.example", &values)
            .assert_value()
            .is_none()
    );
    let values = BTreeMap::from([
        ("GH_HOST".to_owned(), "private.example".to_owned()),
        ("gh_enterprise_token".to_owned(), "ignored-token".to_owned()),
        (
            "GITHUB_ENTERPRISE_TOKEN".to_owned(),
            "canonical-token".to_owned(),
        ),
    ]);
    assert_eq!(
        environment_token(GITHUB, "private.example", &values)
            .assert_value()
            .as_deref(),
        Some("canonical-token")
    );
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
