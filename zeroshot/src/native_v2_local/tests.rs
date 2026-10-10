use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

use super::*;
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_candidate::test_support::{TestDirectory, full_graph, git, success_node};

fn local_request(run_id: RunId) -> PreparedRunRequest {
    let intent = serde_json::from_value(json!({
        "title": "Hermetic local preparation",
        "graph": full_graph(vec![success_node()]),
        "initialInput": null,
        "runtime": {
            "harness": "codex",
            "provider": "openai",
            "size": "small",
            "nodes": {}
        },
        "submissionKey": "hermetic-local-preparation"
    }))
    .assert_value();
    PreparedRunRequest {
        run_id,
        intent,
        connections: BTreeMap::new(),
        github_token: Some("github-token".to_owned()),
        source: None,
        profile: None,
    }
}

fn worker_step() -> Value {
    json!({
        "kind": "step",
        "name": "worker",
        "worker": "agent.worker@1",
        "instructions": "Implement the worker node.",
        "input": { "kind": "null" },
        "output": { "kind": "null" },
        "inputBindings": [],
        "writeBindings": [],
        "timeoutMs": 1000,
        "attempts": 1
    })
}

fn reviewer_step() -> Value {
    json!({
        "kind": "verifier",
        "name": "reviewer",
        "worker": "agent.reviewer@1",
        "instructions": "Review the worker node.",
        "input": { "kind": "null" },
        "output": { "kind": "null" },
        "inputBindings": [],
        "writeBindings": [],
        "timeoutMs": 1000,
        "attempts": 1,
        "signals": {},
        "diagnostic": { "kind": "null" }
    })
}

async fn admitted_with(runtime: Value, mut children: Vec<Value>) -> AdmittedRun {
    children.push(success_node());
    let submission = serde_json::from_value(json!({
        "title": "Local harness lanes",
        "graph": full_graph(children),
        "initialInput": null,
        "runtime": runtime,
        "source": {
            "repository": "open-engine/zeroshot",
            "branch": "main",
            "revision": "0123456789abcdef0123456789abcdef01234567"
        },
        "submissionKey": "local-harness-lanes"
    }))
    .assert_value();
    NativeV2Admission.admit(submission).await.assert_value()
}

async fn admitted_for(harness: &str, provider: &str) -> AdmittedRun {
    admitted_with(
        json!({
            "harness": harness,
            "provider": provider,
            "size": "small",
            "nodes": {
                "worker": { "kind": "agent", "model": "provider-owned-model" }
            }
        }),
        vec![worker_step()],
    )
    .await
}

fn local_environment(root: &TestDirectory) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "HOME".to_owned(),
            root.child("home").to_string_lossy().into_owned(),
        ),
        ("PATH".to_owned(), "/custom/bin".to_owned()),
        (
            "CODEX_HOME".to_owned(),
            root.child("codex").to_string_lossy().into_owned(),
        ),
        (
            "COPILOT_HOME".to_owned(),
            root.child("copilot").to_string_lossy().into_owned(),
        ),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
    ])
}

fn single_lane(lanes: Vec<NativeV2HarnessConfig>) -> NativeV2HarnessConfig {
    let count = lanes.len();
    let Ok([lane]) = <[NativeV2HarnessConfig; 1]>::try_from(lanes) else {
        panic!("expected exactly one lane configuration, got {count}");
    };
    lane
}

fn assert_local_harness_paths(
    actual: (&Path, &Path, &str, Option<&Path>),
    expected: (&Path, &Path, &Path),
) {
    let (actual_workspace, actual_runtime_home, actual_search_path, actual_home) = actual;
    let (workspace, runtime_home, home) = expected;
    assert_eq!(actual_workspace, workspace);
    assert_eq!(actual_runtime_home, runtime_home);
    assert_eq!(actual_search_path, "/custom/bin");
    assert_eq!(actual_home, Some(home));
}

#[test]
fn local_preparation_snapshots_git_identity_and_rejects_ambiguous_sources() {
    let root = TestDirectory::new("local-git-preparation");
    git(root.path(), &["init", "--initial-branch=main"]);
    git(root.path(), &["config", "user.name", "Zeroshot Test"]);
    git(
        root.path(),
        &["config", "user.email", "zeroshot@example.invalid"],
    );
    std::fs::write(root.child("README.md"), b"local fixture\n").assert_value();
    git(root.path(), &["add", "README.md"]);
    git(root.path(), &["commit", "-m", "fixture"]);
    git(
        root.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/open-engine/zeroshot.git",
        ],
    );
    let nested = root.child("nested");
    std::fs::create_dir(&nested).assert_value();
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000041");

    let prepared =
        prepare_local_run(local_request(run_id.clone()), &nested, Path::new("git")).assert_value();
    assert_eq!(prepared.run_id, run_id);
    assert_eq!(prepared.delivery_run_id, prepared.run_id);
    assert_eq!(
        prepared.workspace,
        std::fs::canonicalize(root.path()).assert_value()
    );
    assert_eq!(
        prepared.submission.source.repository.as_str(),
        "open-engine/zeroshot"
    );
    assert_eq!(prepared.submission.source.branch.as_str(), "main");
    assert_eq!(prepared.github_token.as_deref(), Some("github-token"));

    git(root.path(), &["checkout", "--detach"]);
    assert!(matches!(
        local_resolved_source(&nested, Path::new("git")),
        Err(LocalCompositionError::DetachedHead)
    ));
    git(root.path(), &["checkout", "main"]);
    git(
        root.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "https://example.invalid/open-engine/zeroshot.git",
        ],
    );
    assert!(matches!(
        local_resolved_source(&nested, Path::new("git")),
        Err(LocalCompositionError::RepositoryIdentity)
    ));
}

#[tokio::test]
async fn local_harness_contract_materializes_each_native_lane_without_processes() {
    let root = TestDirectory::new("local-harness-contract");
    let workspace = root.child("workspace");
    let runtime_home = root.child("runtime");
    let home = root.child("home");
    let environment = local_environment(&root);

    match single_lane(
        local_lanes(
            &admitted_for("copilot", "github").await,
            &workspace,
            &runtime_home,
            &environment,
        )
        .assert_value(),
    ) {
        NativeV2HarnessConfig::Copilot(config) => {
            assert_local_harness_paths(
                (
                    &config.workspace,
                    &config.runtime_home,
                    &config.search_path,
                    config.local_user.as_ref().map(|user| user.home.as_path()),
                ),
                (&workspace, &runtime_home, &home),
            );
        }
        _ => panic!("expected Copilot harness"),
    }
    match single_lane(
        local_lanes(
            &admitted_for("codex", "openai").await,
            &workspace,
            &runtime_home,
            &environment,
        )
        .assert_value(),
    ) {
        NativeV2HarnessConfig::Codex(config) => {
            assert_eq!(config.executable, PathBuf::from("codex"));
            assert_eq!(config.workspace, workspace);
            assert_eq!(config.runtime_home, runtime_home);
            assert_eq!(
                config.native_environment.get("PATH").map(String::as_str),
                Some("/custom/bin")
            );
            assert_eq!(config.local_user.assert_value().home, home);
        }
        _ => panic!("expected Codex harness"),
    }
    match single_lane(
        local_lanes(
            &admitted_for("claude", "anthropic").await,
            &workspace,
            &runtime_home,
            &environment,
        )
        .assert_value(),
    ) {
        NativeV2HarnessConfig::Claude(config) => {
            assert_eq!(config.workspace, workspace);
            assert_eq!(config.runtime_home, runtime_home);
            assert_eq!(config.local_user_home, Some(home));
            assert_eq!(config.executable, "claude");
        }
        _ => panic!("expected Claude harness"),
    }
}

#[tokio::test]
async fn local_lanes_build_one_configuration_per_effective_lane_from_shared_inputs() {
    let root = TestDirectory::new("local-harness-lanes");
    let workspace = root.child("workspace");
    let runtime_home = root.child("runtime");
    let home = root.child("home");
    let environment = local_environment(&root);
    let admitted = admitted_with(
        json!({
            "harness": "codex",
            "provider": "openai",
            "size": "small",
            "nodes": {
                "worker": { "kind": "agent", "model": "provider-owned-model" },
                "reviewer": {
                    "kind": "agent",
                    "lane": { "harness": "claude", "provider": "anthropic" },
                    "model": "provider-owned-model"
                }
            }
        }),
        vec![worker_step(), reviewer_step()],
    )
    .await;

    let lanes = local_lanes(&admitted, &workspace, &runtime_home, &environment).assert_value();
    let [
        NativeV2HarnessConfig::Codex(codex),
        NativeV2HarnessConfig::Claude(claude),
    ] = lanes.as_slice()
    else {
        panic!("expected exactly a Codex and then a Claude configuration");
    };
    assert_eq!(
        codex.provider,
        openengine_cluster_protocol::CodexProvider::OpenAi
    );
    assert_eq!(
        claude.provider,
        openengine_cluster_protocol::ClaudeProvider::Anthropic
    );
    assert_eq!(codex.executable, PathBuf::from("codex"));
    assert_eq!(claude.executable, "claude");
    for (lane_workspace, lane_runtime_home) in [
        (&codex.workspace, &codex.runtime_home),
        (&claude.workspace, &claude.runtime_home),
    ] {
        assert_eq!(lane_workspace, &workspace);
        assert_eq!(lane_runtime_home, &runtime_home);
    }
    assert_eq!(codex.search_path, "/custom/bin");
    assert_eq!(
        claude.base_environment.clone_values().get("PATH"),
        Some(&codex.search_path)
    );
    assert_eq!(
        codex.local_user.as_ref().map(|user| user.home.as_path()),
        Some(home.as_path())
    );
    assert_eq!(claude.local_user_home.as_deref(), Some(home.as_path()));
}

#[tokio::test]
async fn local_lanes_are_empty_for_a_plan_without_agent_nodes() {
    let root = TestDirectory::new("local-harness-no-agents");
    let admitted = admitted_with(
        json!({
            "harness": "codex",
            "provider": "openai",
            "size": "small",
            "nodes": {}
        }),
        Vec::new(),
    )
    .await;

    let lanes = local_lanes(
        &admitted,
        &root.child("workspace"),
        &root.child("runtime"),
        &local_environment(&root),
    )
    .assert_value();
    assert!(lanes.is_empty());
}

#[test]
fn local_search_path_prefers_the_captured_nonempty_path() {
    let captured = LocalHarnessEnvironment::new(BTreeMap::from([(
        "PATH".to_owned(),
        "/custom/bin".to_owned(),
    )]));
    assert_eq!(local_search_path(&captured), "/custom/bin");

    for environment in [
        BTreeMap::new(),
        BTreeMap::from([("PATH".to_owned(), String::new())]),
    ] {
        let environment = LocalHarnessEnvironment::new(environment);
        assert_eq!(
            local_search_path(&environment),
            default_search_path(&environment)
        );
    }
}

fn joined_search_path(entries: &[&Path]) -> BTreeMap<String, String> {
    let path = std::env::join_paths(entries).assert_value();
    BTreeMap::from([("PATH".to_owned(), path.to_string_lossy().into_owned())])
}

#[test]
fn an_absolute_search_path_does_not_depend_on_the_working_directory() {
    let root = TestDirectory::new("search-path-absolute");
    let absolute = joined_search_path(&[root.path(), &root.child("bin")]);
    assert!(!search_path_depends_on_working_directory(&absolute));

    for environment in [
        BTreeMap::new(),
        BTreeMap::from([("PATH".to_owned(), String::new())]),
    ] {
        assert!(!search_path_depends_on_working_directory(&environment));
    }
}

#[test]
fn an_empty_or_relative_search_path_entry_depends_on_the_working_directory() {
    let root = TestDirectory::new("search-path-relative");
    for entry in ["", "bin"] {
        let environment = joined_search_path(&[Path::new(entry), root.path()]);
        assert!(search_path_depends_on_working_directory(&environment));
    }
}

fn codex_plan(nodes: Value) -> RuntimePlan {
    serde_json::from_value(json!({
        "harness": "codex",
        "provider": "openai",
        "size": "small",
        "nodes": nodes
    }))
    .assert_value()
}

fn mixed_lane_plan() -> RuntimePlan {
    codex_plan(json!({
        "worker": { "kind": "agent", "model": "provider-owned-model" },
        "reviewer": {
            "kind": "agent",
            "lane": { "harness": "claude", "provider": "anthropic" },
            "model": "provider-owned-model"
        }
    }))
}

fn search_directory(root: &TestDirectory) -> (PathBuf, BTreeMap<String, String>) {
    let bin = root.child("bin");
    std::fs::create_dir(&bin).assert_value();
    let environment = BTreeMap::from([("PATH".to_owned(), bin.to_string_lossy().into_owned())]);
    (bin, environment)
}

fn install_executable(directory: &Path, name: &str) {
    #[cfg(unix)]
    openengine_cluster_testkit::fixture::write_executable(
        &directory.join(name),
        "#!/bin/sh\nexit 0\n",
        0o755,
    )
    .assert_value();
    #[cfg(windows)]
    std::fs::write(directory.join(format!("{name}.cmd")), "@exit /b 0\r\n").assert_value();
}

#[test]
fn lane_executables_pass_when_every_lane_finds_its_executable() {
    let root = TestDirectory::new("lane-executables-present");
    let (bin, environment) = search_directory(&root);
    install_executable(&bin, "codex");
    install_executable(&bin, "claude");

    check_lane_executables(&mixed_lane_plan(), &environment, root.path()).assert_value();
}

#[test]
fn a_missing_lane_executable_names_the_first_such_lane_and_its_executable() {
    let root = TestDirectory::new("lane-executables-missing");
    let (bin, environment) = search_directory(&root);

    let error =
        check_lane_executables(&mixed_lane_plan(), &environment, root.path()).assert_error();
    assert_eq!(
        error.to_string(),
        "lane codex/openai needs the `codex` executable on PATH"
    );

    install_executable(&bin, "codex");
    let error =
        check_lane_executables(&mixed_lane_plan(), &environment, root.path()).assert_error();
    assert!(matches!(
        error,
        LocalCompositionError::MissingLaneExecutable {
            lane: RuntimeLane::Claude { .. },
            executable: "claude",
        }
    ));
    assert_eq!(
        error.to_string(),
        "lane claude/anthropic needs the `claude` executable on PATH"
    );
}

#[test]
fn a_plan_without_agent_nodes_needs_no_lane_executable() {
    let root = TestDirectory::new("lane-executables-none");
    let (_bin, environment) = search_directory(&root);

    check_lane_executables(&codex_plan(json!({})), &environment, root.path()).assert_value();
}

#[cfg(unix)]
#[test]
fn an_empty_path_entry_finds_lane_executables_in_the_directory_the_harness_starts_in() {
    let root = TestDirectory::new("lane-executables-empty-entry");
    let (bin, _) = search_directory(&root);
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let environment = BTreeMap::from([("PATH".to_owned(), format!(":{}", bin.display()))]);

    let error = check_lane_executables(&mixed_lane_plan(), &environment, &workspace).assert_error();
    assert_eq!(
        error.to_string(),
        "lane codex/openai needs the `codex` executable on PATH"
    );

    install_executable(&workspace, "codex");
    install_executable(&workspace, "claude");
    check_lane_executables(&mixed_lane_plan(), &environment, &workspace).assert_value();
}

#[test]
fn parses_canonical_github_remote_forms() {
    for remote in [
        "https://github.com/open-engine/zeroshot.git",
        "ssh://git@github.com/open-engine/zeroshot.git",
        "git@github.com:open-engine/zeroshot.git",
    ] {
        assert_eq!(
            github_repository(remote).as_deref(),
            Some("open-engine/zeroshot")
        );
    }
    assert!(github_repository("https://example.com/open-engine/zeroshot.git").is_none());
    assert!(github_repository("https://github.com/extra/open-engine/zeroshot").is_none());
}

#[test]
fn local_preparation_rejects_target_hooks_before_resolving_source_or_installing_anything() {
    for field in ["setup", "startup"] {
        let mut request = local_request(RunId::new("target-only-hooks"));
        request.intent.environment =
            Some(serde_json::from_value(json!({field: "touch must-not-run"})).assert_value());
        assert!(matches!(
            prepare_local_run(
                request,
                Path::new("/nonexistent"),
                Path::new("/nonexistent")
            ),
            Err(LocalCompositionError::PreparationRequiresTarget)
        ));
    }
}
