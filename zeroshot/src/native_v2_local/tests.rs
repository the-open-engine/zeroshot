#[cfg(unix)]
use openengine_cluster_testkit::assertions::AssertError;
use openengine_cluster_testkit::assertions::AssertValue;
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
        git_credentials: None,
        cwd: None,
        source_connection: None,
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
fn local_preparation_snapshots_optional_git_identity_and_accepts_generic_sources() {
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
        prepared
            .submission
            .source
            .as_ref()
            .unwrap()
            .repository
            .as_str(),
        "open-engine/zeroshot"
    );
    assert_eq!(
        prepared.submission.source.as_ref().unwrap().branch.as_str(),
        "main"
    );
    assert_eq!(prepared.github_token.as_deref(), Some("github-token"));

    git(root.path(), &["checkout", "--detach"]);
    assert!(
        local_resolved_source(&nested, Path::new("git"))
            .assert_value()
            .1
            .is_none()
    );
    assert!(matches!(
        resolved_git_source(root.path(), Path::new("git"), None),
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
    let source = local_resolved_source(&nested, Path::new("git"))
        .assert_value()
        .1
        .unwrap();
    let remote = source.remote.unwrap();
    assert_eq!(
        remote.url.as_str(),
        "https://example.invalid/open-engine/zeroshot.git"
    );
    assert!(remote.forge.is_none());
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
            assert_eq!(config.local_user_home, Some(home.clone()));
            assert_eq!(config.executable, "claude");
        }
        _ => panic!("expected Claude harness"),
    }
    match single_lane(
        local_lanes(
            &admitted_for("pi", "anthropic").await,
            &workspace,
            &runtime_home,
            &environment,
        )
        .assert_value(),
    ) {
        NativeV2HarnessConfig::Pi(config) => {
            assert_eq!(
                config.provider,
                crate::native_v2_contract::PiProvider::Anthropic
            );
            assert_eq!(config.executable, "pi");
            assert_eq!(config.workspace, workspace);
            assert_eq!(config.runtime_home, runtime_home);
            // A native-local lane reuses the invoking user's home so Pi finds `~/.pi/agent`.
            assert_eq!(config.local_user_home, Some(home.clone()));
        }
        _ => panic!("expected Pi harness"),
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

// Only Unix restricts lookup to the captured PATH; Windows retains native fallback searches.
#[cfg(unix)]
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

fn forge_source(url: &str, provider: &str) -> ResolvedSource {
    serde_json::from_value(json!({
        "repository": "open-engine/zeroshot",
        "branch": "main",
        "revision": "0123456789abcdef0123456789abcdef01234567",
        "remote": {
            "url": url,
            "forge": { "provider": provider, "repository": "open-engine/zeroshot" }
        }
    }))
    .assert_value()
}

fn github_source() -> ResolvedSource {
    forge_source(
        "https://github.com/open-engine/zeroshot.git",
        "source.github",
    )
}

fn delivery_submission(source: &ResolvedSource) -> RunSubmission {
    serde_json::from_value(json!({
        "title": "Local delivery programs",
        "graph": full_graph(vec![
            crate::native_v2_candidate::test_support::git_delivery_node(),
            success_node(),
        ]),
        "initialInput": null,
        "runtime": {
            "harness": "codex",
            "provider": "openai",
            "size": "small",
            "nodes": { "deliver": { "kind": "git_delivery" } }
        },
        "source": source,
        "submissionKey": "local-delivery-programs"
    }))
    .assert_value()
}

#[cfg(unix)]
#[test]
fn delivery_pins_git_and_the_github_cli_that_the_captured_search_path_finds() {
    let root = TestDirectory::new("delivery-programs-pinned");
    let (bin, environment) = search_directory(&root);
    install_executable(&bin, "git");
    install_executable(&bin, "gh");

    let required = crate::native_v2_delivery::DeliveryMode::PullRequest.required_capabilities();
    let programs = local_delivery_programs(&github_source(), required, &environment, root.path())
        .assert_value();
    assert_eq!(programs.git, bin.join("git"));
    assert_eq!(programs.gh, bin.join("gh"));
}

// Only Unix restricts lookup to the captured PATH; Windows retains native fallback searches.
#[cfg(unix)]
#[test]
fn github_delivery_without_gh_or_git_on_the_captured_search_path_names_the_missing_program() {
    let root = TestDirectory::new("delivery-programs-missing");
    let (bin, environment) = search_directory(&root);
    let submission = delivery_submission(&github_source());

    install_executable(&bin, "git");
    let error = check_delivery_executables(&submission, &environment, root.path()).assert_error();
    assert!(matches!(
        error,
        LocalCompositionError::MissingDeliveryExecutable { executable: "gh" }
    ));
    assert_eq!(
        error.to_string(),
        "Git delivery needs the `gh` executable on PATH"
    );

    std::fs::remove_file(bin.join("git")).assert_value();
    install_executable(&bin, "gh");
    let error = check_delivery_executables(&submission, &environment, root.path()).assert_error();
    assert_eq!(
        error.to_string(),
        "Git delivery needs the `git` executable on PATH"
    );

    install_executable(&bin, "git");
    check_delivery_executables(&submission, &environment, root.path()).assert_value();
}

#[test]
fn only_github_forge_delivery_needs_the_github_cli() {
    use crate::native_v2_delivery::DeliveryMode;
    use crate::source_code_provider::adapters::contributed::registration_tests::{
        HTTPS_REMOTE, PROVIDER,
    };

    let root = TestDirectory::new("delivery-programs-forge");
    let (bin, environment) = search_directory(&root);
    install_executable(&bin, "git");

    for (source, required) in [
        (github_source(), DeliveryMode::Push.required_capabilities()),
        (
            forge_source(HTTPS_REMOTE, PROVIDER),
            DeliveryMode::Merge.required_capabilities(),
        ),
    ] {
        let programs =
            local_delivery_programs(&source, required, &environment, root.path()).assert_value();
        assert_eq!(programs.gh, Path::new("gh"));
    }

    let without_delivery = RunSubmission {
        graph: full_graph(vec![success_node()]),
        ..delivery_submission(&github_source())
    };
    check_delivery_executables(&without_delivery, &BTreeMap::new(), root.path()).assert_value();
}

#[test]
fn parses_canonical_github_remote_forms() {
    for remote in [
        "https://github.com/open-engine/zeroshot.git",
        "ssh://git@github.com/open-engine/zeroshot.git",
        "git@github.com:open-engine/zeroshot.git",
    ] {
        assert_eq!(
            crate::source_code_provider::adapters::core::github_repository_reference(remote)
                .as_deref(),
            Some("open-engine/zeroshot")
        );
    }
    assert!(
        crate::source_code_provider::adapters::core::github_repository_reference(
            "https://example.com/open-engine/zeroshot.git"
        )
        .is_none()
    );
    assert!(
        crate::source_code_provider::adapters::core::github_repository_reference(
            "https://github.com/extra/open-engine/zeroshot"
        )
        .is_none()
    );
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

#[test]
fn relative_harness_home_overrides_resolve_against_the_invoking_directory() {
    let invoking = Path::new("/invoking/dir");
    let mut environment = LocalHarnessEnvironment::new(BTreeMap::from([
        ("CODEX_HOME".to_owned(), "codex".to_owned()),
        ("CLAUDE_CONFIG_DIR".to_owned(), "claude".to_owned()),
        ("COPILOT_HOME".to_owned(), "copilot".to_owned()),
        (
            "COPILOT_PROVIDERS_CONFIG".to_owned(),
            "providers.json".to_owned(),
        ),
        ("PI_CODING_AGENT_DIR".to_owned(), "pi".to_owned()),
        ("PATH".to_owned(), "/usr/bin".to_owned()),
    ]));
    resolve_relative_home_overrides(&mut environment, invoking).assert_value();

    for (name, relative) in [
        ("CODEX_HOME", "codex"),
        ("CLAUDE_CONFIG_DIR", "claude"),
        ("COPILOT_HOME", "copilot"),
        ("COPILOT_PROVIDERS_CONFIG", "providers.json"),
        ("PI_CODING_AGENT_DIR", "pi"),
    ] {
        let expected = invoking.join(relative);
        assert_eq!(
            environment.get(name).map(String::as_str),
            expected.to_str(),
            "{name} must resolve against the invoking directory"
        );
    }
    // An unrelated name, and an already-absolute override, are left untouched.
    assert_eq!(
        environment.get("PATH").map(String::as_str),
        Some("/usr/bin")
    );

    let mut absolute = LocalHarnessEnvironment::new(BTreeMap::from([(
        "PI_CODING_AGENT_DIR".to_owned(),
        "/absolute/pi".to_owned(),
    )]));
    resolve_relative_home_overrides(&mut absolute, invoking).assert_value();
    assert_eq!(
        absolute.get("PI_CODING_AGENT_DIR").map(String::as_str),
        Some("/absolute/pi")
    );
}

#[test]
fn pi_agent_home_shorthand_uses_the_captured_user_home() {
    let invoking = Path::new("/invoking/dir");
    let home = if cfg!(windows) {
        Path::new(r"C:\Users\captured")
    } else {
        Path::new("/captured/home")
    };
    for (authored, relative) in [("~", ""), ("~/.pi/custom", ".pi/custom")] {
        let mut environment = LocalHarnessEnvironment::new(BTreeMap::from([
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            (
                "USERPROFILE".to_owned(),
                home.to_string_lossy().into_owned(),
            ),
            ("PI_CODING_AGENT_DIR".to_owned(), authored.to_owned()),
        ]));
        resolve_relative_home_overrides(&mut environment, invoking).assert_value();
        assert_eq!(
            environment.get("PI_CODING_AGENT_DIR").map(PathBuf::from),
            Some(home.join(relative)),
            "{authored} must keep Pi's native home expansion"
        );
    }
    assert_eq!(
        pi_agent_directory_path("~otheruser/.pi", Some(home)).assert_value(),
        PathBuf::from("~otheruser/.pi")
    );
    let backslash = pi_agent_directory_path(r"~\.pi\custom", Some(home)).assert_value();
    assert_eq!(
        backslash,
        if cfg!(windows) {
            home.join(r".pi\custom")
        } else {
            PathBuf::from(r"~\.pi\custom")
        }
    );
}

#[test]
fn pi_agent_directory_file_urls_keep_their_native_encoding() {
    let authored = if cfg!(windows) {
        "file:///C:/Users/captured/.pi/custom%20agent%23test"
    } else {
        "file:///captured/home/.pi/custom%20agent%23test"
    };
    let mut environment = LocalHarnessEnvironment::new(BTreeMap::from([(
        "PI_CODING_AGENT_DIR".to_owned(),
        authored.to_owned(),
    )]));
    resolve_relative_home_overrides(&mut environment, Path::new("/invoking/dir")).assert_value();
    assert_eq!(
        environment.get("PI_CODING_AGENT_DIR").map(String::as_str),
        Some(authored)
    );
}

#[test]
fn a_bare_repository_is_a_plain_folder_but_cannot_supply_git_delivery() {
    let root = TestDirectory::new("bare-local-source");
    git(root.path(), &["init", "--bare", "--initial-branch", "main"]);
    let (workspace, source) = local_resolved_source(root.path(), Path::new("git")).assert_value();
    assert_eq!(workspace, std::fs::canonicalize(root.path()).assert_value());
    assert!(source.is_none());
    assert!(matches!(
        resolved_git_source(root.path(), Path::new("git"), None),
        Err(LocalCompositionError::DeliverySourceRequired)
    ));
}

#[test]
fn selected_local_cwd_discovers_a_registered_contributed_forge() {
    use crate::source_code_provider::adapters::contributed::registration_tests::{
        HTTPS_REMOTE, SSH_REMOTE, assert_source,
    };
    let repo = crate::native_v2_candidate::test_support::TestGitRepository::delivery();
    let nested = repo.workspace.join("nested");
    std::fs::create_dir(&nested).assert_value();
    let invoking = TestDirectory::new("contribution-invoker");
    for remote in [HTTPS_REMOTE, SSH_REMOTE] {
        git(&repo.workspace, &["remote", "set-url", "origin", remote]);
        let mut request = local_request(RunId::new("contribution-discovery"));
        request.cwd = Some(nested.clone());
        let (workspace, source) =
            prepare_local_source(&request, invoking.path(), Path::new("git")).assert_value();
        assert_eq!(
            workspace,
            std::fs::canonicalize(&repo.workspace).assert_value()
        );
        let source = source.assert_value();
        assert_source(&source, remote);
        assert_eq!(source.revision.as_str(), repo.base);
        crate::source_code_provider::adapters::core::validate_delivery_source(
            &source,
            &[crate::source_code_provider::SourceCapability::PullRequest],
        )
        .assert_value();
    }
}

#[test]
fn local_git_source_uses_remote_upstream_and_rejects_ambiguous_or_stale_selection() {
    let repo = crate::native_v2_candidate::test_support::TestGitRepository::delivery();
    git(&repo.workspace, &["branch", "--unset-upstream"]);
    git(
        &repo.workspace,
        &[
            "remote",
            "set-url",
            "origin",
            "https://forge.example/team/primary.git",
        ],
    );
    let mirror = "ssh://git@forge.example/team/mirror.git";
    git(&repo.workspace, &["remote", "add", "mirror", mirror]);
    assert!(matches!(
        resolved_git_source(&repo.workspace, Path::new("git"), None),
        Err(LocalCompositionError::AmbiguousRemote)
    ));
    assert!(
        local_resolved_source(&repo.workspace, Path::new("git"))
            .assert_value()
            .1
            .is_none()
    );

    git(&repo.workspace, &["config", "branch.main.remote", "mirror"]);
    git(
        &repo.workspace,
        &["config", "branch.main.merge", "refs/heads/release"],
    );
    let connection = openengine_cluster_protocol::ConnectionKey::new("source-only").assert_value();
    let source = resolved_git_source(&repo.workspace, Path::new("git"), Some(connection.clone()))
        .assert_value();
    assert_eq!(source.repository.as_str(), mirror);
    assert_eq!(source.branch.as_str(), "release");
    assert_eq!(source.revision.as_str(), repo.base);
    let remote = source.remote.assert_value();
    assert_eq!(remote.url.as_str(), mirror);
    assert_eq!(remote.connection, Some(connection));
    assert!(remote.forge.is_none());

    git(&repo.workspace, &["config", "branch.main.remote", "gone"]);
    assert!(matches!(
        resolved_git_source(&repo.workspace, Path::new("git"), None),
        Err(LocalCompositionError::RepositoryIdentity)
    ));
    assert!(
        local_resolved_source(&repo.workspace, Path::new("git"))
            .assert_value()
            .1
            .is_none()
    );
    git(&repo.workspace, &["config", "branch.main.remote", "."]);
    assert!(matches!(
        resolved_git_source(&repo.workspace, Path::new("git"), None),
        Err(LocalCompositionError::AmbiguousRemote)
    ));
    git(&repo.workspace, &["remote", "remove", "origin"]);
    let source = resolved_git_source(&repo.workspace, Path::new("git"), None).assert_value();
    assert_eq!(source.branch.as_str(), "main");
    assert_eq!(source.remote.assert_value().url.as_str(), mirror);

    git(&repo.workspace, &["config", "branch.main.remote", "mirror"]);
    git(
        &repo.workspace,
        &["config", "branch.main.merge", "refs/tags/release"],
    );
    let source = resolved_git_source(&repo.workspace, Path::new("git"), None).assert_value();
    assert_eq!(source.branch.as_str(), "main");
}

#[test]
fn local_git_source_keeps_long_transport_identity_stable_without_losing_locator() {
    let repo = crate::native_v2_candidate::test_support::TestGitRepository::delivery();
    let remote = format!("https://forge.example/{}/project.git", "nested/".repeat(50));
    git(&repo.workspace, &["remote", "set-url", "origin", &remote]);
    let source = resolved_git_source(&repo.workspace, Path::new("git"), None).assert_value();
    assert!(source.repository.as_str().len() <= 255);
    assert_eq!(source.remote.as_ref().assert_value().url.as_str(), remote);
    let repeated = resolved_git_source(&repo.workspace, Path::new("git"), None).assert_value();
    assert_eq!(source.repository, repeated.repository);
    let other = format!("https://forge.example/{}/other.git", "nested/".repeat(50));
    git(&repo.workspace, &["remote", "set-url", "origin", &other]);
    let changed = resolved_git_source(&repo.workspace, Path::new("git"), None).assert_value();
    assert_ne!(source.repository, changed.repository);
    assert_eq!(changed.remote.assert_value().url.as_str(), other);
}

#[cfg(unix)]
#[test]
fn local_git_metadata_rejects_invalid_records_and_bounds_untrusted_output() {
    let root = TestDirectory::new("bounded-local-git-output");
    let program = root.child("git-metadata-fixture");
    for script in [
        "printf ignored; exit 1",
        "printf '\\377'",
        "head -c 16385 /dev/zero",
    ] {
        openengine_cluster_testkit::fixture::write_executable(
            &program,
            format!("#!/bin/sh\n{script}\n"),
            0o700,
        )
        .assert_value();
        assert!(git_output(&program, root.path(), &[]).is_err());
    }
    for script in [
        "printf ''",
        "printf 'first\\nsecond\\n'",
        "printf 'first\\000second'",
    ] {
        openengine_cluster_testkit::fixture::write_executable(
            &program,
            format!("#!/bin/sh\n{script}\n"),
            0o700,
        )
        .assert_value();
        assert!(git_line(&program, root.path(), &[]).is_err());
    }
    openengine_cluster_testkit::fixture::write_executable(
        &program,
        "#!/bin/sh\nprintf 'valid-metadata\\r\\n'\n",
        0o700,
    )
    .assert_value();
    assert_eq!(
        git_line(&program, root.path(), &[]).assert_value(),
        "valid-metadata"
    );
}

#[path = "tests/comments.rs"]
mod comments;
