use openengine_cluster_protocol::IdempotencyKey;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;
use super::*;

use crate::native_v2_cli::tests::support::prepared_review_fixture as prepared;

fn identity() -> SnapshotIdentity {
    SnapshotIdentity {
        request: ReviewResolutionRequest {
            url: "https://github.com/acme/project/issues/42".into(),
            branch: None,
            revision: None,
        },
        workspace: None,
        authored_digest: "authored-config".into(),
    }
}

#[tokio::test]
async fn retry_cache_reuses_exact_source_and_refuses_changed_authored_configuration() {
    let directory = tempfile::tempdir().assert_value();
    let key = IdempotencyKey::new("same-submission").assert_value();
    let digest = cache_key("cloud-login-one", &key).assert_value();
    let cache = ReviewCache::open_at(directory.path().to_owned(), digest.clone(), identity())
        .await
        .assert_value();
    assert!(cache.read().assert_value().is_none());
    cache.write(&prepared()).assert_value();
    drop(cache);
    let cache = ReviewCache::open_at(directory.path().to_owned(), digest.clone(), identity())
        .await
        .assert_value();
    assert_eq!(cache.read().assert_value().assert_value(), prepared());
    drop(cache);
    let mut changed = identity();
    changed.authored_digest = "changed-runtime-or-title".into();
    let cache = ReviewCache::open_at(directory.path().to_owned(), digest.clone(), changed)
        .await
        .assert_value();
    assert!(cache.read().is_err());
    drop(cache);
    let mut changed = identity();
    changed.request.url = "https://github.com/acme/project/issues/43".into();
    let cache = ReviewCache::open_at(directory.path().to_owned(), digest, changed)
        .await
        .assert_value();
    assert!(cache.read().is_err());
}

#[test]
fn local_repository_validation_distinguishes_fork_identity_from_unrelated_checkout() {
    let issue = prepared().initial_input;
    assert!(
        validate_local_repository(
            &issue,
            "origin\thttps://github.com/other/project.git (fetch)"
        )
        .is_err()
    );
    validate_local_repository(&issue, "origin\tgit@github.com:acme/project.git (fetch)")
        .assert_value();
    let mut fork = issue;
    fork["context"]["reviewScope"]["headRepository"] = json!("contributor/fork");
    validate_local_repository(
        &fork,
        "origin\thttps://github.com/contributor/fork.git (fetch)",
    )
    .assert_value();
}

fn run_and_profile() -> (RunCommand, super::super::ResolvedRunProfile) {
    let command = crate::native_v2_cli::parse_native_v2_args(
        [
            "run",
            "--title",
            "Review",
            "--target",
            "cloud",
            "--task-from",
            "https://github.com/acme/project/issues/42",
        ]
        .map(std::ffi::OsString::from),
    )
    .assert_value();
    let crate::native_v2_cli::NativeV2CliCommand::Run(run) = command else {
        panic!("expected run");
    };
    let graph = crate::native_v2_cli::BuiltinGraphTemplate::IssueReview
        .materialize(crate::native_v2_cli::TemplateDelivery::Comment)
        .assert_value();
    let runtime=serde_json::from_value(json!({"harness":"codex","provider":"openai","size":"small","nodes":{"reviewer":{"kind":"agent","model":"original-model"},"deliver":{"kind":"comment_delivery","connections":{"github":["GH_TOKEN"]}}}})).assert_value();
    (
        *run,
        super::super::ResolvedRunProfile {
            graph,
            runtime,
            remote_selector: None,
        },
    )
}

#[test]
fn retry_identity_includes_title_runtime_and_concrete_environment_bytes() {
    let (mut run, mut profile) = run_and_profile();
    let original = snapshot_identity(&run, &profile, None)
        .assert_value()
        .authored_digest;
    run.title = openengine_cluster_protocol::RunTitle::new("Changed title").assert_value();
    assert_ne!(
        original,
        snapshot_identity(&run, &profile, None)
            .assert_value()
            .authored_digest
    );
    run.title = openengine_cluster_protocol::RunTitle::new("Review").assert_value();
    let binding = profile
        .runtime
        .nodes_mut()
        .get_mut(&openengine_cluster_protocol::NodeName::new("reviewer").assert_value())
        .unwrap();
    if let openengine_cluster_protocol::NodeRuntimeBinding::Agent { model, .. } = binding {
        *model = openengine_cluster_protocol::ModelId::new("different-model").assert_value();
    }
    assert_ne!(
        original,
        snapshot_identity(&run, &profile, None)
            .assert_value()
            .authored_digest
    );
    let file = tempfile::NamedTempFile::new().assert_value();
    run.environment = Some(crate::native_v2_cli::RunEnvironmentInput::File(
        file.path().to_owned(),
    ));
    std::fs::write(file.path(), r#"{"variables":{"MODE":"first"}}"#).assert_value();
    let captured = super::super::SubmissionInput::capture(&run).assert_value();
    let first = snapshot_identity(&run, &profile, captured.environment.as_ref())
        .assert_value()
        .authored_digest;
    std::fs::write(file.path(), r#"{"variables":{"MODE":"second"}}"#).assert_value();
    assert_eq!(
        first,
        snapshot_identity(&run, &profile, captured.environment.as_ref())
            .assert_value()
            .authored_digest
    );
    let changed = super::super::SubmissionInput::capture(&run).assert_value();
    assert_ne!(
        first,
        snapshot_identity(&run, &profile, changed.environment.as_ref())
            .assert_value()
            .authored_digest
    );
}

fn git(directory: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(directory)
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .args(args)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .assert_value()
        .trim()
        .to_owned()
}

fn initialize_repository(directory: &std::path::Path) {
    git(directory, &["init", "-b", "main"]);
    git(
        directory,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/project.git",
        ],
    );
}

#[tokio::test]
async fn local_review_checks_head_and_tracked_files_without_changing_the_worktree() {
    let directory = tempfile::tempdir().assert_value();
    initialize_repository(directory.path());
    std::fs::write(directory.path().join("code.txt"), "original").assert_value();
    git(directory.path(), &["add", "code.txt"]);
    git(
        directory.path(),
        &["-c", "commit.gpgsign=false", "commit", "-m", "fixture"],
    );
    let head = git(directory.path(), &["rev-parse", "HEAD"]);
    let (mut run, profile) = run_and_profile();
    run.target = None;
    run.cwd = Some(directory.path().to_owned());
    let mut snapshot = prepared();
    local_snapshot(&run, &mut snapshot, false)
        .await
        .assert_value();
    let input = super::super::SubmissionInput {
        review: Some(snapshot),
        environment: None,
    };
    let mut params =
        super::super::prepare_submission_with_environment(&run, profile, |_| None, input)
            .assert_value();
    validate_local(&mut params).await.assert_value();
    assert_eq!(
        params.source.as_ref().unwrap().resolved.revision.as_str(),
        head
    );
    params.intent.initial_input["context"]["reviewScope"]["headRevision"] = json!("b".repeat(40));
    assert!(validate_local(&mut params).await.is_err());
    assert_eq!(git(directory.path(), &["rev-parse", "HEAD"]), head);
    params.intent.initial_input["context"]["reviewScope"] = json!({});
    std::fs::write(directory.path().join("code.txt"), "user change").assert_value();
    assert!(validate_local(&mut params).await.is_err());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("code.txt")).assert_value(),
        "user change"
    );
}

#[tokio::test]
async fn cached_local_source_cannot_follow_a_later_head() {
    let directory = tempfile::tempdir().assert_value();
    initialize_repository(directory.path());
    git(
        directory.path(),
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    let (mut run, _) = run_and_profile();
    run.target = None;
    run.cwd = Some(directory.path().to_owned());
    let mut snapshot = prepared();
    local_snapshot(&run, &mut snapshot, false)
        .await
        .assert_value();
    let original = snapshot.source.revision.clone();
    let cache_dir = tempfile::tempdir().assert_value();
    let cache = ReviewCache::open_at(cache_dir.path().to_owned(), "local".into(), identity())
        .await
        .assert_value();
    cache.write(&snapshot).assert_value();
    drop(cache);
    git(
        directory.path(),
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "later",
        ],
    );
    let cache = ReviewCache::open_at(cache_dir.path().to_owned(), "local".into(), identity())
        .await
        .assert_value();
    let mut saved = cache.read().assert_value().assert_value();
    let error = local_snapshot(&run, &mut saved, true).await.unwrap_err();
    assert!(error.to_string().contains("HEAD changed"));
    assert_eq!(saved.source.revision, original);
    assert_ne!(
        git(directory.path(), &["rev-parse", "HEAD"]),
        original.as_str()
    );
}
