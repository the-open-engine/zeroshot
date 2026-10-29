use super::*;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

#[test]
fn ambient_credentials_are_selected_for_the_exact_provider_group() {
    let values = BTreeMap::from([
        ("GH_TOKEN".to_owned(), "public-token".to_owned()),
        (
            "GH_ENTERPRISE_TOKEN".to_owned(),
            "enterprise-token".to_owned(),
        ),
        ("GITLAB_TOKEN".to_owned(), "gitlab-token".to_owned()),
    ]);
    assert_eq!(
        environment_token(GITHUB, "github.com", &values)
            .assert_value()
            .as_deref(),
        Some("public-token")
    );
    assert_eq!(
        environment_token(GITHUB, "example.ghe.com", &values)
            .assert_value()
            .as_deref(),
        Some("public-token")
    );
    assert_eq!(
        environment_token(GITHUB, "git.example.test", &values)
            .assert_value()
            .as_deref(),
        Some("enterprise-token")
    );
    assert_eq!(
        environment_token(GITLAB, "gitlab.com", &values)
            .assert_value()
            .as_deref(),
        Some("gitlab-token")
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
