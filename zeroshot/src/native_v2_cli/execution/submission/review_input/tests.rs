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
    let cache = ReviewCache::open_at(directory.path().join("cache"), digest.clone(), identity())
        .await
        .assert_value();
    assert!(cache.read().assert_value().is_none());
    cache.write(&prepared()).assert_value();
    drop(cache);
    let cache = ReviewCache::open_at(directory.path().join("cache"), digest.clone(), identity())
        .await
        .assert_value();
    assert_eq!(cache.read().assert_value().assert_value(), prepared());
    drop(cache);
    let mut changed = identity();
    changed.authored_digest = "changed-runtime-or-title".into();
    let cache = ReviewCache::open_at(directory.path().join("cache"), digest.clone(), changed)
        .await
        .assert_value();
    assert!(cache.read().is_err());
    drop(cache);
    let mut changed = identity();
    changed.request.url = "https://github.com/acme/project/issues/43".into();
    let cache = ReviewCache::open_at(directory.path().join("cache"), digest, changed)
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
    let cache = ReviewCache::open_at(cache_dir.path().join("cache"), "local".into(), identity())
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
    let cache = ReviewCache::open_at(cache_dir.path().join("cache"), "local".into(), identity())
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

#[tokio::test]
async fn retry_cache_keeps_its_snapshot_when_a_new_value_exceeds_the_bound() {
    let directory = tempfile::tempdir().assert_value();
    let cache = ReviewCache::open_at(directory.path().join("cache"), "bounded".into(), identity())
        .await
        .assert_value();
    let original = prepared();
    cache.write(&original).assert_value();
    let original_bytes = std::fs::read(&cache.destination).assert_value();
    let mut oversized = original.clone();
    oversized.initial_input["context"]["body"] = json!("x".repeat(4 * 1024 * 1024));
    let error = cache.write(&oversized).unwrap_err();
    assert!(error.to_string().contains("exceeds its bound"));
    assert_eq!(
        std::fs::read(&cache.destination).assert_value(),
        original_bytes
    );
    assert_eq!(cache.read().assert_value(), Some(original));
    assert_eq!(std::fs::read_dir(&cache.root).assert_value().count(), 2);
}

#[tokio::test]
async fn cached_preparation_never_refetches_or_silently_replaces_corrupted_state() {
    let directory = tempfile::tempdir().assert_value();
    let cache = ReviewCache::open_at(
        directory.path().join("cache"),
        "prepared".into(),
        identity(),
    )
    .await
    .assert_value();
    let (run, profile) = run_and_profile();
    let backend = crate::native_v2_cli::tests::support::FakeBackend::with_review(prepared());
    let first = cache
        .prepare(&run, &profile.runtime, &backend)
        .await
        .assert_value();
    assert_eq!(backend.calls().len(), 1);
    let unavailable = crate::native_v2_cli::tests::support::FakeBackend::default();
    assert_eq!(
        cache
            .prepare(&run, &profile.runtime, &unavailable)
            .await
            .assert_value(),
        first
    );
    assert!(unavailable.calls().is_empty());
    for corrupt in ["{", r#"{"identity":null,"prepared":null}"#] {
        std::fs::write(&cache.destination, corrupt).assert_value();
        let error = cache
            .prepare(&run, &profile.runtime, &backend)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("saved review input is invalid"));
        assert_eq!(backend.calls().len(), 1);
        assert_eq!(
            std::fs::read_to_string(&cache.destination).assert_value(),
            corrupt
        );
    }
}

#[tokio::test]
async fn failed_preparation_leaves_the_submission_key_available_for_a_valid_retry() {
    let directory = tempfile::tempdir().assert_value();
    let cache = ReviewCache::open_at(directory.path().join("cache"), "failed".into(), identity())
        .await
        .assert_value();
    let (run, profile) = run_and_profile();
    let unavailable = crate::native_v2_cli::tests::support::FakeBackend::default();
    assert!(
        cache
            .prepare(&run, &profile.runtime, &unavailable)
            .await
            .is_err()
    );
    assert!(cache.read().assert_value().is_none());
    let backend = crate::native_v2_cli::tests::support::FakeBackend::with_review(prepared());
    let snapshot = cache
        .prepare(&run, &profile.runtime, &backend)
        .await
        .assert_value();
    assert_eq!(snapshot, prepared());
    assert_eq!(cache.read().assert_value(), Some(snapshot));
}

#[cfg(unix)]
#[tokio::test]
async fn cache_files_cannot_redirect_reads_or_locks_to_another_file() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().assert_value();
    let external = directory.path().join("external");
    std::fs::write(&external, "caller-owned").assert_value();
    let cache_root = directory.path().join("cache");
    let cache = ReviewCache::open_at(cache_root.clone(), "symlink".into(), identity())
        .await
        .assert_value();
    symlink(&external, &cache.destination).assert_value();
    assert!(cache.read().is_err());
    drop(cache);
    std::fs::remove_file(cache_root.join("symlink.lock")).assert_value();
    symlink(&external, cache_root.join("symlink.lock")).assert_value();
    assert!(
        ReviewCache::open_at(cache_root, "symlink".into(), identity())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(external).assert_value(),
        "caller-owned"
    );
}

#[tokio::test]
async fn local_preparation_refuses_unborn_checkout_and_absent_admitted_source() {
    let directory = tempfile::tempdir().assert_value();
    initialize_repository(directory.path());
    let (mut run, profile) = run_and_profile();
    run.target = None;
    run.cwd = Some(directory.path().to_owned());
    let mut snapshot = prepared();
    assert!(local_snapshot(&run, &mut snapshot, false).await.is_err());
    let mut params = super::super::prepare_submission_with_environment(
        &run,
        profile,
        |_| None,
        super::super::SubmissionInput {
            review: Some(snapshot),
            environment: None,
        },
    )
    .assert_value();
    params.source = None;
    assert!(
        validate_local(&mut params)
            .await
            .unwrap_err()
            .to_string()
            .contains("source is unavailable")
    );
    assert!(validate_local_repository(&json!({"url":null}), "").is_err());
    assert!(
        local_git(&directory.path().join("absent"), &["rev-parse", "HEAD"])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn local_submission_reuses_frozen_input_and_refuses_changed_intent_before_fetching() {
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
            "fixture",
        ],
    );
    let (mut run, profile) = run_and_profile();
    run.target = None;
    run.cwd = Some(directory.path().to_owned());
    let key = IdempotencyKey::new(format!("cache-test-{}", uuid::Uuid::now_v7())).assert_value();
    run.submission_key = Some(key.clone());
    let backend = crate::native_v2_cli::tests::support::FakeBackend::with_review(prepared());
    let original = resolve(&run, &profile, &backend)
        .await
        .assert_value()
        .review
        .assert_value();
    assert_eq!(backend.calls().len(), 1);
    assert_eq!(
        original.source.revision.as_str(),
        git(directory.path(), &["rev-parse", "HEAD"])
    );
    let mut changed = prepared();
    changed.initial_input["context"]["body"] = json!("edited after first resolution");
    let backend = crate::native_v2_cli::tests::support::FakeBackend::with_review(changed);
    assert_eq!(
        resolve(&run, &profile, &backend)
            .await
            .assert_value()
            .review,
        Some(original)
    );
    assert!(backend.calls().is_empty());
    run.title = openengine_cluster_protocol::RunTitle::new("Changed authored title").assert_value();
    let error = resolve(&run, &profile, &backend).await.err().assert_value();
    assert!(
        error
            .to_string()
            .contains("different review input or authored configuration")
    );
    assert!(backend.calls().is_empty());
    let digest = cache_key("local", &key).assert_value();
    let root = crate::native_v2_cli::default_local_state_root()
        .assert_value()
        .join("review-inputs");
    for extension in ["json", "lock"] {
        std::fs::remove_file(root.join(format!("{digest}.{extension}"))).assert_value();
    }
}

#[tokio::test]
async fn ordinary_input_skips_metadata_and_unavailable_cache_authority_is_not_invented() {
    let (mut run, profile) = run_and_profile();
    let backend = crate::native_v2_cli::tests::support::FakeBackend::with_review(prepared());
    run.input = RunInput::File("caller-owned-input.json".into());
    assert!(
        resolve(&run, &profile, &backend)
            .await
            .assert_value()
            .review
            .is_none()
    );
    assert!(backend.calls().is_empty());
    run.input = RunInput::TaskFrom(identity().request.url);
    run.submission_key = Some(IdempotencyKey::new("no-cache-authority").assert_value());
    assert_eq!(
        resolve(&run, &profile, &backend)
            .await
            .assert_value()
            .review,
        Some(prepared())
    );
    assert_eq!(backend.calls().len(), 1);
}

#[tokio::test]
async fn invalid_workspace_and_blocked_cache_root_fail_before_metadata_resolution() {
    let directory = tempfile::tempdir().assert_value();
    let (mut run, profile) = run_and_profile();
    let backend = crate::native_v2_cli::tests::support::FakeBackend::with_review(prepared());
    run.target = None;
    run.cwd = Some(directory.path().join("absent"));
    run.submission_key = Some(IdempotencyKey::new("invalid-workspace").assert_value());
    assert!(resolve(&run, &profile, &backend).await.is_err());
    assert!(backend.calls().is_empty());
    let blocked = directory.path().join("blocked-cache");
    std::fs::write(&blocked, "caller-owned").assert_value();
    assert!(
        ReviewCache::open_at(blocked.clone(), "key".into(), identity())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(blocked).assert_value(),
        "caller-owned"
    );
}

#[tokio::test]
async fn failed_cache_commit_preserves_the_destination_and_removes_temporary_state() {
    let directory = tempfile::tempdir().assert_value();
    let cache = ReviewCache::open_at(directory.path().join("cache"), "blocked".into(), identity())
        .await
        .assert_value();
    std::fs::create_dir(&cache.destination).assert_value();
    let sentinel = cache.destination.join("caller-owned");
    std::fs::write(&sentinel, "preserve").assert_value();
    assert!(cache.write(&prepared()).is_err());
    assert_eq!(std::fs::read_to_string(sentinel).assert_value(), "preserve");
    assert_eq!(std::fs::read_dir(&cache.root).assert_value().count(), 2);
}
