use super::*;
use crate::native_v2_candidate::test_support::git;
use crate::native_v2_delivery::{DeliveryMode, TrustedGitHubTokenFile};
use crate::native_v2_local::{LocalCompositionError, validate_remote_base_branch};
use openengine_cluster_protocol::{ConnectionKey, NodeRuntimeBinding, StaticConnectionValues};

struct Fixture {
    backend: LocalCliBackend,
    queries: PathBuf,
    root: TestDirectory,
}

impl Fixture {
    fn new(remote_reply: &str) -> Self {
        let root = TestDirectory::new("pb");
        let workspace = root.child("w");
        std::fs::create_dir(&workspace).assert_value();
        initialize_local_repository(&workspace);
        let queries = root.child("queries");
        let git_program = root.child("git");
        openengine_cluster_testkit::fixture::write_executable(
            &git_program,
            format!(
                "#!/bin/sh\nreference=\nfor arg in \"$@\"; do\n case \"$arg\" in refs/heads/*) reference=\"$arg\";; esac\ndone\nfor arg in \"$@\"; do\n if [ \"$arg\" = ls-remote ]; then\n  printf '%s\\n' \"$reference\" >> '{}'\n  {remote_reply}\n fi\ndone\nexec /usr/bin/git \"$@\"\n",
                queries.display(),
            ),
            0o700,
        )
        .assert_value();
        Self {
            backend: LocalCliBackend::new(
                root.child("s"),
                PathBuf::from("/bin/true"),
                workspace,
                git_program,
            )
            .with_ready_timeout(Duration::from_millis(10)),
            queries,
            root,
        }
    }

    fn request(&self, mode: DeliveryMode) -> PreparedRunRequest {
        let mut request = prepared_request(
            RunId::new("0199f33f-3b44-7d21-9000-0000000000c1"),
            "remote-base-preflight",
        );
        let mut node = crate::native_v2_candidate::test_support::git_delivery_node();
        node["worker"] = json!(mode.worker_ref());
        node["output"] = serde_json::to_value(
            crate::native_v2_delivery::contract::delivery_result_schema(mode).assert_value(),
        )
        .assert_value();
        node["signals"] = json!({
            "delivery": crate::native_v2_delivery::delivery_signal_labels(mode).assert_value()
        });
        request.intent.graph = full_graph(vec![node, success_node()]);
        request.intent.runtime.nodes_mut().insert(
            NodeName::new("deliver").assert_value(),
            NodeRuntimeBinding::GitDelivery {
                connections: Default::default(),
                pull_request_feedback: Default::default(),
            },
        );
        request
    }

    async fn preflight(&self, prepared: &PreparedLocalRun) -> Result<(), LocalCompositionError> {
        validate_remote_base_branch(
            prepared,
            &self.backend.git_program,
            &self.backend.state_root,
        )
        .await
    }

    fn observations(&self) -> Vec<String> {
        std::fs::read_to_string(&self.queries)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

const MAIN_REF: &str = "if [ \"$reference\" = refs/heads/main ]; then printf '0123456789abcdef0123456789abcdef01234567\\t%s\\n' \"$reference\"; fi\nexit 0";

fn authenticated_ref(basic: &str) -> String {
    format!(
        "if [ \"${{GIT_CONFIG_VALUE_2-}}\" != 'AUTHORIZATION: basic {basic}' ]; then exit 128; fi\n{MAIN_REF}",
    )
}

fn declare_connections(request: &mut PreparedRunRequest, fields: &[&str]) {
    request.intent.runtime.nodes_mut().insert(
        NodeName::new("deliver").assert_value(),
        serde_json::from_value(json!({
            "kind": "git_delivery", "connections": {"delivery": fields}
        }))
        .assert_value(),
    );
}

// These preflight contracts must not depend on the invoking user's native forge login.
pub(super) fn with_declared_github_token(mut request: PreparedRunRequest) -> PreparedRunRequest {
    declare_connections(&mut request, &["GH_TOKEN"]);
    request.connections.insert(
        ConnectionKey::new("delivery").assert_value(),
        serde_json::from_value(json!({"GH_TOKEN":"preflight-token"})).assert_value(),
    );
    request
}

fn assert_controller_start(result: Result<RunId, NativeV2CliError>) {
    assert!(
        matches!(result, Err(NativeV2CliError::Local(_))),
        "preflight should pass and reach the inert controller fixture: {result:?}"
    );
}

#[tokio::test]
async fn fresh_branch_is_rejected_until_its_upstream_selects_a_remote_base() {
    for mode in [DeliveryMode::PullRequest, DeliveryMode::Merge] {
        let fixture = Fixture::new(MAIN_REF);
        let workspace = &fixture.backend.current_directory;
        git(workspace, &["checkout", "-b", "my-change"]);
        let rejected = fixture
            .backend
            .start_controller(with_declared_github_token(fixture.request(mode)))
            .await;
        assert!(
            matches!(rejected, Err(NativeV2CliError::Usage(ref message))
                if message.contains("remote does not have target branch `my-change`")
                    && message.contains("--set-upstream-to=origin/main")),
            "a fresh local branch must fail before controller startup: {rejected:?}"
        );
        assert!(!fixture.backend.state_root.join("runs").exists());
        git(
            workspace,
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
        git(workspace, &["branch", "--set-upstream-to=origin/main"]);
        assert_controller_start(
            fixture
                .backend
                .start_controller(with_declared_github_token(fixture.request(mode)))
                .await,
        );
        assert_eq!(
            fixture.observations(),
            ["refs/heads/my-change", "refs/heads/main"]
        );
    }
}

#[tokio::test]
async fn delivery_connection_token_reaches_preflight_and_outranks_trusted_token() {
    // x-access-token:connection-token
    let reply = authenticated_ref("eC1hY2Nlc3MtdG9rZW46Y29ubmVjdGlvbi10b2tlbg==");
    for stored in [false, true] {
        let fixture = Fixture::new(&reply);
        let mut request = fixture.request(DeliveryMode::PullRequest);
        declare_connections(&mut request, &["GH_TOKEN"]);
        request.github_token = Some("stale-trusted-token".to_owned());
        let values: StaticConnectionValues = serde_json::from_value(json!({
            "GH_TOKEN": "connection-token"
        }))
        .assert_value();
        let key = ConnectionKey::new("delivery").assert_value();
        if stored {
            LocalConnectionStore::new(fixture.backend.state_root.clone())
                .set(ConnectionSetRequest {
                    key,
                    scope: openengine_cluster_protocol::ConnectionScope::User,
                    values,
                })
                .assert_value();
        } else {
            request.connections.insert(key, values);
        }
        assert_controller_start(fixture.backend.start_controller(request).await);
        assert_eq!(fixture.observations(), ["refs/heads/main"]);
    }
}

#[tokio::test]
async fn preflight_reuses_snapshot_and_preserves_trusted_token_file_precedence() {
    // x-access-token:file-token
    let reply = format!(
        "if [ \"${{PREFLIGHT_SNAPSHOT-}}\" != captured ]; then exit 128; fi\n{}",
        authenticated_ref("eC1hY2Nlc3MtdG9rZW46ZmlsZS10b2tlbg=="),
    );
    let fixture = Fixture::new(&reply);
    // Reserve trusted authority before preparation, then attach its renewable file below.
    // A real token-file selection reserves this same lane before native login materialization.
    let mut request = fixture.request(DeliveryMode::Merge);
    request.github_token = Some("file-token".to_owned());
    let mut prepared = fixture.backend.prepare_run(request).await.assert_value();
    prepared.github_token = None;
    prepared
        .native_environment
        .insert("PREFLIGHT_SNAPSHOT".to_owned(), "captured".to_owned());
    let path = fixture.root.child("token");
    private_file(&path, FileAccess::CreateNew)
        .assert_value()
        .write_all(b"file-token\n")
        .assert_value();
    prepared.github_token_file =
        Some(TrustedGitHubTokenFile::new(path.clone(), &prepared.workspace).assert_value());
    fixture.preflight(&prepared).await.assert_value();
    std::fs::remove_file(path).assert_value();
    assert!(matches!(
        fixture.preflight(&prepared).await,
        Err(LocalCompositionError::GitHubTokenFile)
    ));
    assert_eq!(fixture.observations(), ["refs/heads/main"]);
    prepared.git_credentials = Some(
        openengine_cluster_protocol::GitTransportCredentials::Https {
            username: "x-access-token".to_owned(),
            password: "file-token".to_owned(),
        },
    );
    fixture.preflight(&prepared).await.assert_value();
    prepared.git_credentials = None;
    prepared.github_token = Some("file-token".to_owned());
    fixture.preflight(&prepared).await.assert_value();
    // A declared node credential must also suppress an unavailable trusted file.
    let mut request = fixture.request(DeliveryMode::Merge);
    declare_connections(&mut request, &["GH_TOKEN"]);
    request.connections =
        serde_json::from_value(json!({"delivery":{"GH_TOKEN":"file-token"}})).assert_value();
    let selected = fixture.backend.prepare_run(request).await.assert_value();
    prepared.environment = selected.environment;
    prepared.submission = selected.submission;
    prepared.github_token = None;
    fixture.preflight(&prepared).await.assert_value();
}

#[tokio::test]
async fn remote_access_failure_is_distinct_from_a_missing_branch() {
    let fixture = Fixture::new("exit 128");
    let result = fixture
        .backend
        .start_controller(with_declared_github_token(
            fixture.request(DeliveryMode::Merge),
        ))
        .await;
    assert!(
        matches!(result, Err(NativeV2CliError::Usage(ref message))
            if message.contains("could not verify target branch `main`")
                && message.contains("connectivity and credentials")),
        "remote access failures must not claim a branch is missing: {result:?}"
    );
    assert!(!fixture.backend.state_root.join("runs").exists());
}

#[tokio::test]
async fn preflight_honors_source_transport_credentials_before_delivery_fields() {
    for source_connection in [false, true] {
        // source-user:source-password or delivery-user:delivery-password
        let basic = if source_connection {
            "c291cmNlLXVzZXI6c291cmNlLXBhc3N3b3Jk"
        } else {
            "ZGVsaXZlcnktdXNlcjpkZWxpdmVyeS1wYXNzd29yZA=="
        };
        let fixture = Fixture::new(&authenticated_ref(basic));
        let mut request = fixture.request(DeliveryMode::Merge);
        declare_connections(&mut request, &["GIT_USERNAME", "GIT_PASSWORD", "GH_TOKEN"]);
        request.connections = serde_json::from_value(json!({
            "delivery":{"GIT_USERNAME":"delivery-user", "GIT_PASSWORD":"delivery-password", "GH_TOKEN":"forge-token"}
        })).assert_value();
        if source_connection {
            let key = ConnectionKey::new("source").assert_value();
            request.source_connection = Some(key.clone());
            request.connections.insert(
                key,
                serde_json::from_value(json!({
                    "GIT_USERNAME":"source-user", "GIT_PASSWORD":"source-password"
                }))
                .assert_value(),
            );
        }
        assert_controller_start(fixture.backend.start_controller(request).await);
        assert_eq!(fixture.observations(), ["refs/heads/main"]);
    }
}

#[tokio::test]
async fn another_forge_uses_git_transport_without_reading_github_authority() {
    let fixture = Fixture::new(MAIN_REF);
    git(
        &fixture.backend.current_directory,
        &[
            "remote",
            "set-url",
            "origin",
            crate::source_code_provider::adapters::contributed::registration_tests::HTTPS_REMOTE,
        ],
    );
    let mut request = fixture.request(DeliveryMode::PullRequest);
    declare_connections(&mut request, &["OTHER_FORGE_TOKEN"]);
    request.connections =
        serde_json::from_value(json!({"delivery":{"OTHER_FORGE_TOKEN":"forge-token"}}))
            .assert_value();
    let mut prepared = fixture.backend.prepare_run(request).await.assert_value();
    prepared.github_token_file = Some(
        TrustedGitHubTokenFile::new(fixture.root.child("missing-token"), &prepared.workspace)
            .assert_value(),
    );
    validate_remote_base_branch(
        &prepared,
        &fixture.backend.git_program,
        &fixture.backend.state_root,
    )
    .await
    .assert_value();
    assert_eq!(fixture.observations(), ["refs/heads/main"]);
}

#[tokio::test]
async fn accepted_pr_submission_returns_without_remote_access() {
    let fixture = Fixture::new("exit 128");
    let prepared = fixture
        .backend
        .prepare_run(with_declared_github_token(
            fixture.request(DeliveryMode::PullRequest),
        ))
        .await
        .assert_value();
    create_contract_run(&fixture.backend, &prepared.run_id, &prepared.submission).await;
    fixture
        .backend
        .write_recovery_document(
            &prepared.run_id,
            &recovery_document(
                prepared.submission.clone(),
                prepared.workspace.clone(),
                None,
            ),
        )
        .assert_value();
    assert_eq!(
        fixture
            .backend
            .start_controller(with_declared_github_token(
                fixture.request(DeliveryMode::PullRequest),
            ))
            .await
            .assert_value(),
        prepared.run_id
    );
    assert!(fixture.observations().is_empty());
}

#[tokio::test]
async fn push_only_submission_does_not_check_for_a_remote_base() {
    let fixture = Fixture::new("exit 128");
    assert_controller_start(
        fixture
            .backend
            .start_controller(fixture.request(DeliveryMode::Push))
            .await,
    );
    assert!(fixture.observations().is_empty());
}

#[tokio::test]
async fn preflight_resolves_relative_search_paths_against_the_selected_workspace() {
    let fixture = Fixture::new(MAIN_REF);
    let mut prepared = fixture
        .backend
        .prepare_run(with_declared_github_token(
            fixture.request(DeliveryMode::PullRequest),
        ))
        .await
        .assert_value();
    let tools = prepared.workspace.join("preflight-tools");
    std::fs::create_dir(&tools).assert_value();
    std::os::unix::fs::symlink(&fixture.backend.git_program, tools.join("git")).assert_value();
    prepared
        .native_environment
        .insert("PATH".to_owned(), "preflight-tools".to_owned());
    validate_remote_base_branch(&prepared, Path::new("git"), &fixture.backend.state_root)
        .await
        .assert_value();
    assert_eq!(fixture.observations(), ["refs/heads/main"]);
}
