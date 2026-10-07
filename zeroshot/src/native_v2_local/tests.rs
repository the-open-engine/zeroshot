use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

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

async fn admitted_for(harness: &str, provider: &str) -> AdmittedRun {
    let submission = serde_json::from_value(json!({
        "title": format!("Local {harness} harness"),
        "graph": full_graph(vec![success_node()]),
        "initialInput": null,
        "runtime": {
            "harness": harness,
            "provider": provider,
            "size": "small",
            "nodes": {}
        },
        "source": {
            "repository": "open-engine/zeroshot",
            "branch": "main",
            "revision": "0123456789abcdef0123456789abcdef01234567"
        },
        "submissionKey": format!("local-{harness}-harness")
    }))
    .assert_value();
    NativeV2Admission.admit(submission).await.assert_value()
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
    let environment = BTreeMap::from([
        ("HOME".to_owned(), home.to_string_lossy().into_owned()),
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
    ]);

    match local_harness(
        &admitted_for("copilot", "github").await,
        &workspace,
        &runtime_home,
        &environment,
    )
    .assert_value()
    {
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
    match local_harness(
        &admitted_for("codex", "openai").await,
        &workspace,
        &runtime_home,
        &environment,
    )
    .assert_value()
    {
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
    match local_harness(
        &admitted_for("claude", "anthropic").await,
        &workspace,
        &runtime_home,
        &environment,
    )
    .assert_value()
    {
        NativeV2HarnessConfig::Claude(config) => {
            assert_eq!(config.workspace, workspace);
            assert_eq!(config.runtime_home, runtime_home);
            assert_eq!(config.local_user_home, Some(home));
            assert_eq!(config.executable, "claude");
        }
        _ => panic!("expected Claude harness"),
    }
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
