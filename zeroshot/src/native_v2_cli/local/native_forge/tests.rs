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

fn fixture_forge(provider: &str, host: &str) -> SourceForgeIdentity {
    SourceForgeIdentity {
        provider: openengine_cluster_protocol::SourceForgeProviderId::new(provider).assert_value(),
        repository: openengine_cluster_protocol::SourceRepositoryId::new("team/repo")
            .assert_value(),
        base_url: Some(SourceForgeBaseUrl::new(format!("https://{host}")).assert_value()),
    }
}

#[test]
fn one_native_connection_cannot_authorize_different_instances() {
    let key = ConnectionKey::new("delivery").assert_value();
    let mut selected = BTreeMap::new();
    insert_selection(
        &mut selected,
        key.clone(),
        fixture_forge(GITHUB, "one.example.test"),
    )
    .assert_value();
    insert_selection(
        &mut selected,
        key,
        fixture_forge(GITHUB, "two.example.test"),
    )
    .assert_error();
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
        ("GITHUB_TOKEN".to_owned(), "wrong-public-alias".to_owned()),
        (
            "GITHUB_ENTERPRISE_TOKEN".to_owned(),
            "wrong-enterprise-alias".to_owned(),
        ),
        (
            "GITLAB_ACCESS_TOKEN".to_owned(),
            "wrong-gitlab-alias".to_owned(),
        ),
        ("OAUTH_TOKEN".to_owned(), "wrong-oauth-token".to_owned()),
        ("GH_HOST".to_owned(), "ambient.example.test".to_owned()),
        (
            "GITLAB_HOST".to_owned(),
            "https://ambient.example.test/".to_owned(),
        ),
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
[ -z "${GH_TOKEN-}${GITHUB_TOKEN-}${GH_ENTERPRISE_TOKEN-}${GITHUB_ENTERPRISE_TOKEN-}${GITLAB_TOKEN-}${GITLAB_ACCESS_TOKEN-}${OAUTH_TOKEN-}${GH_HOST-}${GITLAB_HOST-}" ] || exit 4
[ "$DBUS_SESSION_BUS_ADDRESS" = native-keyring ] || exit 5
printf '%s\n' private-native-token
"#,
        0o700,
    )
    .assert_value();
    let environment = fixture_environment(root.path());
    let token = token_for(
        &fixture_forge(GITHUB, "git.example.test"),
        NativeContext {
            environment: &environment,
            directory: root.path(),
        },
    )
    .await
    .assert_value();
    assert_eq!(token, "private-native-token");
}

#[cfg(unix)]
#[tokio::test]
async fn native_gitlab_lookup_uses_the_git_credential_protocol() {
    let root = tempfile::tempdir().assert_value();
    openengine_cluster_testkit::fixture::write_executable(
        &root.path().join("glab"),
        r#"#!/bin/sh
[ "$1" = auth ] && [ "$2" = git-credential ] && [ "$3" = get ] || exit 2
[ -z "${GH_TOKEN-}${GITHUB_TOKEN-}${GH_ENTERPRISE_TOKEN-}${GITHUB_ENTERPRISE_TOKEN-}${GITLAB_TOKEN-}${GITLAB_ACCESS_TOKEN-}${OAUTH_TOKEN-}${GH_HOST-}${GITLAB_HOST-}" ] || exit 3
IFS= read -r protocol
IFS= read -r host
[ "$protocol" = protocol=https ] && [ "$host" = host=git.example.test ] || exit 4
/bin/cat >/dev/null || exit 8
printf 'username=oauth2\npassword=private-gitlab-token\n\n'
"#,
        0o700,
    )
    .assert_value();
    let environment = fixture_environment(root.path());
    let token = token_for(
        &fixture_forge(GITLAB, "git.example.test"),
        NativeContext {
            environment: &environment,
            directory: root.path(),
        },
    )
    .await
    .assert_value();
    assert_eq!(token, "private-gitlab-token");
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
    let environment = BTreeMap::from([
        ("GH_HOST".to_owned(), "git.example.test".to_owned()),
        ("GH_ENTERPRISE_TOKEN".to_owned(), "ambient-token".to_owned()),
    ]);
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
    let environment = BTreeMap::from([
        ("GH_HOST".to_owned(), "git.example.test".to_owned()),
        ("GH_ENTERPRISE_TOKEN".to_owned(), "ambient-token".to_owned()),
    ]);
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

#[cfg(unix)]
#[tokio::test]
async fn source_less_comment_destinations_cannot_claim_unselected_ambient_tokens() {
    for (url, field) in [
        ("https://git.example.test/team/repo/issues/17", "GH_TOKEN"),
        ("https://company.ghe.com/team/repo/issues/17", "GH_TOKEN"),
        (
            "https://git.example.test/team/repo/-/issues/17",
            "GITLAB_TOKEN",
        ),
    ] {
        let root = tempfile::tempdir().assert_value();
        let store = LocalConnectionStore::new(root.path().join("state"));
        let mut submission = comment_submission("git.example.test", &[field]);
        submission.initial_input["url"] = serde_json::json!(url);
        assert!(submission.source.is_none());
        for selector_present in [false, true] {
            let mut environment = fixture_environment(root.path());
            if !selector_present {
                environment.remove("GH_HOST");
                environment.remove("GITLAB_HOST");
            }
            let context = NativeContext {
                environment: &environment,
                directory: root.path(),
            };
            let mut supplied = RunConnectionValues::new();
            let error = fill_missing(&submission, &mut supplied, &store, context)
                .await
                .assert_error();
            assert!(
                error.to_string().contains("forge login for"),
                "{url}: {error}"
            );
            assert!(
                supplied.is_empty(),
                "{url} must not receive an unrelated ambient token"
            );
            for (name, secret) in &environment {
                if name.ends_with("TOKEN") {
                    assert!(!error.to_string().contains(secret), "{name}");
                }
            }
        }
    }
}

#[tokio::test]
async fn local_delivery_captures_enterprise_alias_without_using_the_public_token() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalConnectionStore::new(root.path().join("state"));
    let submission = comment_submission("ghe.example.test", &["GH_TOKEN"]);
    let environment = BTreeMap::from([
        ("GH_HOST".to_owned(), "ghe.example.test".to_owned()),
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
        (
            "glab",
            r#"[ "$*" = 'auth git-credential get' ] || exit 5
IFS= read -r protocol
IFS= read -r host
[ "$protocol" = protocol=https ] && [ "$host" = host=forge.example.test ] || exit 6
/bin/cat >/dev/null || exit 7
printf 'password=native-config-token\n\n'"#,
        ),
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
/bin/cat >/dev/null || exit 8
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

fn source_fixture(remote: &str, forge: Option<SourceForgeIdentity>) -> ResolvedSource {
    serde_json::from_value(serde_json::json!({
        "repository": forge.as_ref().map_or("unidentified", |value| value.repository.as_str()),
        "branch":"feature", "revision":"a".repeat(40),
        "remote":{"url":remote,"forge":forge}
    }))
    .assert_value()
}

#[cfg(unix)]
fn explicit_local_command(provider: &str, base: &str) -> crate::native_v2_cli::RunCommand {
    let command = crate::native_v2_cli::parse_native_v2_args(
        [
            "run",
            "--title",
            "Local forge selection",
            "--input",
            "unused.json",
            "--source-provider",
            provider,
            "--source-base-url",
            base,
        ]
        .into_iter()
        .map(std::ffi::OsString::from),
    )
    .assert_value();
    let crate::native_v2_cli::NativeV2CliCommand::Run(command) = command else {
        panic!("fixture must parse as a run");
    };
    *command
}

#[cfg(unix)]
fn local_selection_request(
    source: Option<ResolvedSource>,
) -> crate::native_v2_cli::PreparedRunRequest {
    let submission = comment_submission("github.com", &["GH_TOKEN"]);
    crate::native_v2_cli::PreparedRunRequest {
        run_id: openengine_cluster_protocol::RunId::new("native-source-selection"),
        intent: (&submission).into(),
        connections: RunConnectionValues::new(),
        github_token: None,
        git_credentials: None,
        cwd: None,
        source_connection: None,
        source: source.map(|resolved| crate::native_v2_cli::NamedRunSource {
            resolved,
            dirty: true,
        }),
        profile: None,
    }
}

#[cfg(unix)]
#[test]
fn explicit_local_selection_uses_the_selected_checkout_and_scopes_transport_authority() {
    use crate::native_v2_candidate::test_support::{git, git_output};
    let invoking = std::env::current_dir().assert_value();
    let root = tempfile::tempdir_in(invoking.parent().assert_value()).assert_value();
    git(
        root.path(),
        &["init", "--quiet", "--initial-branch=feature"],
    );
    git(
        root.path(),
        &[
            "-c",
            "user.name=Native Fixture",
            "-c",
            "user.email=native@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "fixture",
        ],
    );
    git(
        root.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://placeholder.example/team/repo.git",
        ],
    );
    let revision = git_output(root.path(), &["rev-parse", "HEAD"]);
    let relative = Path::new("..").join(root.path().file_name().assert_value());
    let key = ConnectionKey::new("source-transport").assert_value();
    for (provider, base, remote, repository) in [
        (
            GITHUB,
            "https://ghe.example.test",
            "https://ghe.example.test/team/repo.git",
            "team/repo",
        ),
        (
            GITLAB,
            "https://git.example.test/install",
            "https://git.example.test/install/team/nested/repo.git",
            "team/nested/repo",
        ),
    ] {
        git(root.path(), &["remote", "set-url", "origin", remote]);
        for cwd in [root.path().to_owned(), relative.clone()] {
            let mut run = explicit_local_command(provider, base);
            run.cwd = Some(cwd);
            run.source_connection = Some(key.clone());
            let mut request = local_selection_request(None);
            select_local_source(&run, &mut request).assert_value();
            let selected = request.source.assert_value();
            assert!(!selected.dirty);
            assert_eq!(selected.resolved.repository.as_str(), repository);
            assert_eq!(selected.resolved.branch.as_str(), "feature");
            assert_eq!(selected.resolved.revision.as_str(), revision);
            let remote = selected.resolved.remote.assert_value();
            assert_eq!(remote.connection, Some(key.clone()));
            let forge = remote.forge.assert_value();
            assert_eq!(forge.provider.as_str(), provider);
            assert_eq!(forge.repository.as_str(), repository);
            assert_eq!(forge.base_url.assert_value().as_str(), base);
        }
    }
}

#[cfg(unix)]
#[test]
fn explicit_local_selection_never_rewrites_named_or_unselected_requests() {
    let original = local_selection_request(Some(source_fixture(
        "https://existing.example/team/repo.git",
        None,
    )));
    for named in [false, true] {
        let mut run = explicit_local_command(GITHUB, "https://ghe.example.test");
        if named {
            run.target = Some("docker".to_owned());
        } else {
            run.source_forge = None;
        }
        let mut request = original.clone();
        select_local_source(&run, &mut request).assert_value();
        assert_eq!(request, original);
    }
}

#[cfg(unix)]
#[test]
fn explicit_local_selection_refuses_plain_folders_and_unbound_or_mismatched_remotes() {
    let root = tempfile::tempdir().assert_value();
    let mut run = explicit_local_command(GITHUB, "https://ghe.example.test");
    run.cwd = Some(root.path().to_owned());
    let mut request = local_selection_request(None);
    let error = select_local_source(&run, &mut request).assert_error();
    assert!(error.to_string().contains("attached Git checkout"));
    assert!(request.source.is_none());
    let mut legacy = source_fixture("https://ghe.example.test/team/repo.git", None);
    legacy.remote = None;
    for source in [
        legacy,
        source_fixture("https://other.example.test/team/repo.git", None),
    ] {
        let original = local_selection_request(Some(source));
        let mut request = original.clone();
        select_local_source(&run, &mut request).assert_error();
        assert_eq!(
            request, original,
            "rejected selection must preserve its captured source"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn source_enrichment_requires_one_native_login_and_preserves_frozen_identity() {
    for (provider, program, response) in [
        (
            GITHUB,
            "gh",
            r#"[ "$*" = 'auth token --hostname forge.example.test:8443' ] || exit 2
printf 'github-host-token\n'"#,
        ),
        (
            GITLAB,
            "glab",
            r#"IFS= read -r protocol
IFS= read -r host
[ "$protocol" = protocol=https ] && [ "$host" = host=forge.example.test:8443 ] || exit 2
/bin/cat >/dev/null || exit 3
printf 'password=gitlab-host-token\n\n'"#,
        ),
    ] {
        let root = tempfile::tempdir().assert_value();
        openengine_cluster_testkit::fixture::write_executable(
            &root.path().join(program),
            format!("#!/bin/sh\n{response}\n"),
            0o700,
        )
        .assert_value();
        let environment = fixture_environment(root.path());
        let mut source = source_fixture("https://forge.example.test:8443/team/repo.git", None);
        enrich_source(
            &mut source,
            NativeContext {
                environment: &environment,
                directory: root.path(),
            },
        )
        .await
        .assert_value();
        assert_eq!(source.repository.as_str(), "team/repo");
        let forge = source
            .remote
            .as_ref()
            .assert_value()
            .forge
            .as_ref()
            .assert_value();
        assert_eq!(forge.provider.as_str(), provider);
        assert_eq!(
            forge.base_url.as_ref().assert_value().as_str(),
            "https://forge.example.test:8443"
        );
        let empty = BTreeMap::new();
        let context = NativeContext {
            environment: &empty,
            directory: root.path(),
        };
        let frozen = source.clone();
        enrich_source(&mut source, context).await.assert_value();
        assert_eq!(
            source, frozen,
            "known source identity must not re-probe native login"
        );
        source.remote = None;
        let legacy = source.clone();
        enrich_source(&mut source, context).await.assert_value();
        assert_eq!(
            source, legacy,
            "source-only history must not acquire a new remote"
        );
    }
}

fn delivery_submission(source: Option<ResolvedSource>) -> RunSubmission {
    use crate::native_v2_candidate::test_support::{full_graph, git_delivery_node, success_node};
    let mut submission = comment_submission("github.com", &["GH_TOKEN"]);
    submission.graph = full_graph(vec![git_delivery_node(), success_node()]);
    submission.initial_input = serde_json::Value::Null;
    submission.runtime.nodes_mut().clear();
    submission.runtime.nodes_mut().insert(
        openengine_cluster_protocol::NodeName::new("deliver").assert_value(),
        serde_json::from_value(serde_json::json!({"kind":"git_delivery"})).assert_value(),
    );
    submission.source = source;
    submission
}

#[test]
fn trusted_local_authority_retains_only_empty_github_com_delivery_requirements() {
    for (provider, host, legacy, trusted, expected) in [
        (GITHUB, "github.com", true, true, serde_json::json!({})),
        (GITHUB, "github.com", false, true, serde_json::json!({})),
        (
            GITHUB,
            "github.com",
            false,
            false,
            serde_json::json!({"github":["GH_TOKEN"]}),
        ),
        (
            GITHUB,
            "ghe.example.test",
            false,
            true,
            serde_json::json!({"github":["GH_TOKEN"]}),
        ),
        (
            GITLAB,
            "gitlab.com",
            false,
            true,
            serde_json::json!({"gitlab":["GITLAB_TOKEN"]}),
        ),
    ] {
        let mut source = source_fixture(
            &format!("https://{host}/team/repo.git"),
            Some(fixture_forge(provider, host)),
        );
        if legacy {
            source.remote = None;
        }
        let mut submission = delivery_submission(Some(source));
        let environment = BTreeMap::new();
        materialize_local_access(
            &mut submission,
            NativeContext {
                environment: &environment,
                directory: Path::new("."),
            },
            trusted,
        )
        .assert_value();
        let binding = submission.runtime.nodes().values().next().assert_value();
        assert_eq!(
            serde_json::to_value(binding.declared_connections()).assert_value(),
            expected
        );
    }
    let mut submission = delivery_submission(None);
    let runtime = submission.runtime.clone();
    let environment = BTreeMap::new();
    let error = materialize_local_access(
        &mut submission,
        NativeContext {
            environment: &environment,
            directory: Path::new("."),
        },
        true,
    )
    .assert_error();
    assert!(matches!(error, NativeV2CliError::InvalidRun(_)));
    assert_eq!(
        submission.runtime, runtime,
        "failed materialization must remain atomic"
    );
}

#[tokio::test]
async fn a_shared_connection_with_additional_fields_cannot_be_filled_from_native_login() {
    let root = tempfile::tempdir().assert_value();
    let store = LocalConnectionStore::new(root.path().join("state"));
    let mut submission = comment_submission("ghe.example.test", &["GH_TOKEN"]);
    submission.runtime.nodes_mut().insert(
        openengine_cluster_protocol::NodeName::new("reviewer").assert_value(),
        serde_json::from_value(serde_json::json!({
            "kind":"agent", "model":"caller-model", "connections":{"work-ghe":["OTHER"]}
        }))
        .assert_value(),
    );
    let environment = BTreeMap::from([
        ("GH_HOST".to_owned(), "ghe.example.test".to_owned()),
        ("GH_ENTERPRISE_TOKEN".to_owned(), "ambient-token".to_owned()),
    ]);
    let supplied = native_connections(&submission, &store, root.path(), &environment).await;
    assert!(
        supplied.is_empty(),
        "native login must not invent another node's missing fields"
    );
    store
        .resolve_submission(&submission, &supplied)
        .assert_error();
}
