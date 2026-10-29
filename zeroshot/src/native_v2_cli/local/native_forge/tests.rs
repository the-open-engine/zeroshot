use super::*;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

#[test]
fn native_source_base_does_not_confuse_ssh_ports_with_web_ports() {
    for (remote, base) in [
        (
            "git@git.example.test:team/repo.git",
            "https://git.example.test",
        ),
        (
            "ssh://git@git.example.test:2222/team/repo.git",
            "https://git.example.test",
        ),
        (
            "https://git.example.test:8443/team/repo.git",
            "https://git.example.test:8443",
        ),
    ] {
        assert_eq!(
            remote_base(&GitRemoteUrl::new(remote).assert_value())
                .assert_value()
                .as_str()
                .trim_end_matches('/'),
            base,
        );
    }
}

#[test]
fn one_native_connection_cannot_authorize_different_instances() {
    let key = ConnectionKey::new("delivery").assert_value();
    let make = |host: &str| SourceForgeIdentity {
        provider: openengine_cluster_protocol::SourceForgeProviderId::new(GITHUB).assert_value(),
        repository: openengine_cluster_protocol::SourceRepositoryId::new("team/repo")
            .assert_value(),
        base_url: Some(SourceForgeBaseUrl::new(format!("https://{host}")).assert_value()),
    };
    let mut selected = BTreeMap::new();
    insert_selection(&mut selected, key.clone(), make("one.example.test")).assert_value();
    insert_selection(&mut selected, key, make("two.example.test")).assert_error();
}

#[cfg(unix)]
fn fixture_environment(root: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("PATH".to_owned(), root.to_string_lossy().into_owned()),
        ("HOME".to_owned(), root.to_string_lossy().into_owned()),
        ("GH_TOKEN".to_owned(), "wrong-public-token".to_owned()),
        (
            "GH_ENTERPRISE_TOKEN".to_owned(),
            "wrong-enterprise-token".to_owned(),
        ),
        ("GITLAB_TOKEN".to_owned(), "wrong-gitlab-token".to_owned()),
        (
            "DBUS_SESSION_BUS_ADDRESS".to_owned(),
            "native-keyring".to_owned(),
        ),
    ])
}

#[cfg(unix)]
#[tokio::test]
async fn native_github_lookup_pins_host_and_keeps_only_native_context() {
    let root = tempfile::tempdir().assert_value();
    openengine_cluster_testkit::fixture::write_executable(
        &root.path().join("gh"),
        r#"#!/bin/sh
[ "$1" = auth ] && [ "$2" = token ] && [ "$3" = --hostname ] || exit 2
[ "$4" = git.example.test ] || exit 3
[ -z "${GH_TOKEN-}${GH_ENTERPRISE_TOKEN-}${GITLAB_TOKEN-}" ] || exit 4
[ "$DBUS_SESSION_BUS_ADDRESS" = native-keyring ] || exit 5
printf '%s\n' private-native-token
"#,
        0o700,
    )
    .assert_value();
    let environment = fixture_environment(root.path());
    let token = process::stored_token(
        GITHUB,
        "git.example.test",
        NativeContext {
            environment: &environment,
            directory: root.path(),
        },
    )
    .await
    .assert_value();
    assert_eq!(token.as_deref(), Some("private-native-token"));
}

#[cfg(unix)]
#[tokio::test]
async fn native_gitlab_lookup_uses_the_git_credential_protocol() {
    let root = tempfile::tempdir().assert_value();
    openengine_cluster_testkit::fixture::write_executable(
        &root.path().join("glab"),
        r#"#!/bin/sh
[ "$1" = auth ] && [ "$2" = git-credential ] && [ "$3" = get ] || exit 2
[ -z "${GH_TOKEN-}${GH_ENTERPRISE_TOKEN-}${GITLAB_TOKEN-}" ] || exit 3
IFS= read -r protocol
IFS= read -r host
[ "$protocol" = protocol=https ] && [ "$host" = host=git.example.test ] || exit 4
printf 'username=oauth2\npassword=private-gitlab-token\n\n'
"#,
        0o700,
    )
    .assert_value();
    let environment = fixture_environment(root.path());
    let token = process::stored_token(
        GITLAB,
        "git.example.test",
        NativeContext {
            environment: &environment,
            directory: root.path(),
        },
    )
    .await
    .assert_value();
    assert_eq!(token.as_deref(), Some("private-gitlab-token"));
}

#[cfg(unix)]
#[tokio::test]
async fn two_native_logins_are_ambiguous_instead_of_guessing_a_provider() {
    let root = tempfile::tempdir().assert_value();
    for (program, response) in [
        ("gh", "printf 'github-token\\n'"),
        (
            "glab",
            "/bin/cat >/dev/null; printf 'password=gitlab-token\\n\\n'",
        ),
    ] {
        openengine_cluster_testkit::fixture::write_executable(
            &root.path().join(program),
            format!("#!/bin/sh\n{response}\n"),
            0o700,
        )
        .assert_value();
    }
    let environment = fixture_environment(root.path());
    let remote = GitRemoteUrl::new("git@git.example.test:team/repo.git").assert_value();
    let error = identify_remote(
        &remote,
        NativeContext {
            environment: &environment,
            directory: root.path(),
        },
    )
    .await
    .assert_error();
    assert!(error.to_string().contains("both gh and glab"), "{error}");
}

fn comment_submission(host: &str, fields: &[&str]) -> RunSubmission {
    let graph = crate::native_v2_cli::BuiltinGraphTemplate::IssueReview
        .materialize(crate::native_v2_cli::TemplateDelivery::Comment)
        .assert_value();
    serde_json::from_value(serde_json::json!({
        "title":"Native comment",
        "graph":graph,
        "initialInput":{"url":format!("https://{host}/team/repo/issues/17"),
            "context":{"title":"Proposal","body":"Review this","reviewScope":{}}},
        "runtime":{"harness":"codex","provider":"openai","size":"small","nodes":{
            "reviewer":{"kind":"agent","model":"caller-model"},
            "deliver":{"kind":"comment_delivery","connections":{"work-ghe":fields}}
        }},
        "submissionKey":"native-comment"
    }))
    .assert_value()
}

async fn native_connections(
    submission: &RunSubmission,
    store: &LocalConnectionStore,
    directory: &Path,
    environment: &BTreeMap<String, String>,
) -> RunConnectionValues {
    let mut supplied = RunConnectionValues::new();
    fill_missing(
        submission,
        &mut supplied,
        store,
        NativeContext {
            environment,
            directory,
        },
    )
    .await
    .assert_value();
    supplied
}

fn values(token: &str) -> StaticConnectionValues {
    StaticConnectionValues::new(BTreeMap::from([(
        EnvironmentVariableName::new("GH_TOKEN").assert_value(),
        token.to_owned(),
    )]))
    .assert_value()
}

#[tokio::test]
async fn explicit_and_stored_connections_win_without_starting_native_auth() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalConnectionStore::new(root.path().join("state"));
    let key = ConnectionKey::new("work-ghe").assert_value();
    store
        .set(openengine_cluster_protocol::ConnectionSetRequest {
            key: key.clone(),
            scope: openengine_cluster_protocol::ConnectionScope::User,
            values: values("stored-token"),
        })
        .assert_value();
    let submission = comment_submission("git.example.test", &["GH_TOKEN"]);
    let environment = BTreeMap::new();
    let context = NativeContext {
        environment: &environment,
        directory: root.path(),
    };
    for (mut supplied, expected) in [
        (
            RunConnectionValues::from([(key.clone(), values("explicit-token"))]),
            "explicit-token",
        ),
        (RunConnectionValues::new(), "stored-token"),
    ] {
        fill_missing(&submission, &mut supplied, &store, context)
            .await
            .assert_value();
        let resolved = store
            .resolve_submission(&submission, &supplied)
            .assert_value()
            .bootstrap_values();
        assert_eq!(resolved[&key], values(expected));
    }
}

#[tokio::test]
async fn partial_explicit_connection_is_not_repaired_by_native_credentials() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalConnectionStore::new(root.path().join("state"));
    let submission = comment_submission("git.example.test", &["GH_TOKEN", "OTHER"]);
    let key = ConnectionKey::new("work-ghe").assert_value();
    let mut supplied = RunConnectionValues::from([(key, values("explicit-token"))]);
    let environment =
        BTreeMap::from([("GH_ENTERPRISE_TOKEN".to_owned(), "ambient-token".to_owned())]);
    let error = fill_missing(
        &submission,
        &mut supplied,
        &store,
        NativeContext {
            environment: &environment,
            directory: root.path(),
        },
    )
    .await
    .assert_error();
    assert!(error.to_string().contains("does not exactly define"));
}

#[cfg(unix)]
#[tokio::test]
async fn source_less_comments_and_resumes_read_fresh_credentials_for_the_destination() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalConnectionStore::new(root.path().join("state"));
    let submission = comment_submission("comment.example.test", &["GH_TOKEN"]);
    assert!(submission.source.is_none());
    let environment = BTreeMap::from([(
        "PATH".to_owned(),
        root.path().to_string_lossy().into_owned(),
    )]);
    for token in ["first-native-token", "rotated-native-token"] {
        openengine_cluster_testkit::fixture::write_executable(
            &root.path().join("gh"),
            format!(
                "#!/bin/sh\n[ \"$4\" = comment.example.test ] || exit 2\nprintf '%s\\n' {token}\n"
            ),
            0o700,
        )
        .assert_value();
        let supplied = native_connections(&submission, &store, root.path(), &environment).await;
        assert_eq!(
            supplied[&ConnectionKey::new("work-ghe").assert_value()],
            values(token)
        );
    }
}

#[tokio::test]
async fn local_delivery_captures_enterprise_alias_without_using_the_public_token() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalConnectionStore::new(root.path().join("state"));
    let submission = comment_submission("ghe.example.test", &["GH_TOKEN"]);
    let environment = BTreeMap::from([
        ("GH_TOKEN".to_owned(), "public-token".to_owned()),
        (
            "GH_ENTERPRISE_TOKEN".to_owned(),
            "enterprise-token".to_owned(),
        ),
    ]);
    let supplied = native_connections(&submission, &store, root.path(), &environment).await;
    assert_eq!(
        supplied[&ConnectionKey::new("work-ghe").assert_value()],
        values("enterprise-token"),
    );
}

#[test]
fn pending_legacy_token_file_keeps_only_empty_github_com_defaults_unmaterialized() {
    let environment = BTreeMap::from([(
        crate::native_v2_delivery::GITHUB_TOKEN_FILE_ENV.to_owned(),
        "/pending/private/token".to_owned(),
    )]);
    for (host, retained) in [("github.com", true), ("ghe.example.test", false)] {
        let mut submission = comment_submission(host, &["GH_TOKEN"]);
        let node = submission
            .runtime
            .nodes_mut()
            .values_mut()
            .find(|binding| matches!(binding, NodeRuntimeBinding::CommentDelivery { .. }))
            .assert_value();
        *node = NodeRuntimeBinding::CommentDelivery {
            connections: openengine_cluster_protocol::DeclaredConnections::empty(),
        };
        materialize_local_access(
            &mut submission,
            NativeContext {
                environment: &environment,
                directory: Path::new("."),
            },
            false,
        )
        .assert_value();
        let node = submission
            .runtime
            .nodes()
            .values()
            .find(|binding| matches!(binding, NodeRuntimeBinding::CommentDelivery { .. }))
            .assert_value();
        assert_eq!(node.declared_connections().is_empty(), retained);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn native_helpers_forward_the_captured_absolute_config_directories() {
    let invoking = tempfile::tempdir().assert_value();
    let workspace = invoking.path().join("selected-workspace");
    std::fs::create_dir(&workspace).assert_value();
    for (program, response) in [
        ("gh", "printf 'native-config-token\\n'"),
        ("glab", "printf 'password=native-config-token\\n\\n'"),
    ] {
        let script = format!(
            r#"#!/bin/sh
[ "$GH_CONFIG_DIR" = "$HOME/config/gh" ] || exit 2
[ "$GLAB_CONFIG_DIR" = "$HOME/config/glab" ] || exit 3
[ "$PWD" = "$HOME/selected-workspace" ] || exit 4
{response}
"#
        );
        openengine_cluster_testkit::fixture::write_executable(
            &invoking.path().join(program),
            script,
            0o700,
        )
        .assert_value();
    }
    let mut environment = fixture_environment(invoking.path());
    environment.insert(
        "GH_CONFIG_DIR".to_owned(),
        invoking
            .path()
            .join("config/gh")
            .to_string_lossy()
            .into_owned(),
    );
    environment.insert(
        "GLAB_CONFIG_DIR".to_owned(),
        invoking
            .path()
            .join("config/glab")
            .to_string_lossy()
            .into_owned(),
    );
    for provider in [GITHUB, GITLAB] {
        let token = process::stored_token(
            provider,
            "forge.example.test",
            NativeContext {
                environment: &environment,
                directory: &workspace,
            },
        )
        .await
        .assert_value();
        assert_eq!(token.as_deref(), Some("native-config-token"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn auto_identified_gitlab_uses_its_own_default_native_host() {
    let remote = GitRemoteUrl::new("git@gitlab.com:team/subgroup/project.git").assert_value();
    let mut forge = crate::source_code_provider::adapters::core::forge_identity(&remote)
        .assert_value()
        .assert_value();
    assert_eq!(forge.provider.as_str(), GITLAB);
    forge.base_url = None;
    let root = tempfile::tempdir().assert_value();
    openengine_cluster_testkit::fixture::write_executable(
        &root.path().join("glab"),
        r#"#!/bin/sh
IFS= read -r protocol
IFS= read -r host
[ "$protocol" = protocol=https ] && [ "$host" = host=gitlab.com ] || exit 2
printf 'password=gitlab-native-token\n\n'
"#,
        0o700,
    )
    .assert_value();
    let environment = BTreeMap::from([(
        "PATH".to_owned(),
        root.path().to_string_lossy().into_owned(),
    )]);
    let token = token_for(
        &forge,
        NativeContext {
            environment: &environment,
            directory: root.path(),
        },
    )
    .await
    .assert_value();
    assert_eq!(token, "gitlab-native-token");
}
