use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::native_v2_candidate::test_support::{TestGitRepository, git, git_output, path_text};

struct ConflictFixture {
    repository: TestGitRepository,
    authority: GhCliDeliveryAuthority,
    request: GitHubConflictRequest,
    target_revision: String,
    git_program: PathBuf,
    gh_program: PathBuf,
}

enum ExpectedCleanup {
    AbortedPendingMerge,
    AlreadyClean,
}

impl ConflictFixture {
    fn new(target_file: &str) -> Self {
        let repository = TestGitRepository::delivery();
        commit_all(&repository.workspace, "worker change");
        let head_revision = git_output(&repository.workspace, &["rev-parse", "HEAD"]);
        let target = repository.root.child("target");
        git(
            repository.root.path(),
            &["clone", path_text(&repository.remote), path_text(&target)],
        );
        fs::write(target.join(target_file), "target\n").assert_value();
        commit_all(&target, "target change");
        git(&target, &["push", "origin", "main"]);
        let target_revision = git_output(&target, &["rev-parse", "HEAD"]);
        Self::with_revisions(repository, head_revision, target_revision)
    }

    fn target_is_ancestor() -> Self {
        let repository = TestGitRepository::delivery();
        commit_all(&repository.workspace, "worker change");
        let head_revision = git_output(&repository.workspace, &["rev-parse", "HEAD"]);
        let target_revision = repository.base.clone();
        Self::with_revisions(repository, head_revision, target_revision)
    }

    fn with_revisions(
        repository: TestGitRepository,
        head_revision: String,
        target_revision: String,
    ) -> Self {
        let git_program = git_wrapper(&repository);
        let gh_program = gh_script(&repository, &target_revision);
        let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
            git_program: git_program.clone(),
            gh_program: gh_program.clone(),
            home_directory: repository.root.path().to_owned(),
            api_deadline: Duration::from_secs(10),
            push_deadline: Duration::from_secs(10),
        });
        let request = GitHubConflictRequest {
            workspace: repository.workspace.clone(),
            review: GitHubReviewReceipt {
                review_id: "17".to_owned(),
                repository: "acme/project".to_owned(),
                target_branch: "main".to_owned(),
                head_branch: "zeroshot/v2-test".to_owned(),
                head_revision,
            },
        };
        Self {
            repository,
            authority,
            request,
            target_revision,
            git_program,
            gh_program,
        }
    }
}

#[tokio::test]
async fn authenticated_target_fetch_leaves_exact_conflict_for_repair() {
    let fixture = ConflictFixture::new("result.txt");

    let outcome = fixture
        .authority
        .materialize_merge_conflict(&fixture.request, GitHubCredential("test-token"))
        .await
        .assert_value();
    let GitHubConflictOutcome::Materialized(materialized) = outcome else {
        panic!("expected a materialized merge conflict");
    };

    assert_eq!(materialized.target_revision, fixture.target_revision);
    assert_eq!(materialized.conflicted_paths, vec!["result.txt"]);
    assert_eq!(
        git_output(&fixture.repository.workspace, &["rev-parse", "MERGE_HEAD"]),
        fixture.target_revision
    );
    assert!(
        fs::read_to_string(fixture.repository.workspace.join("result.txt"))
            .assert_value()
            .contains("<<<<<<<")
    );
    assert!(
        git_output(&fixture.repository.workspace, &["status", "--porcelain=v1"])
            .contains("AA result.txt")
    );
    assert_transport_keeps_credential_ephemeral(&fixture);
}

#[tokio::test]
async fn stale_conflict_observation_restores_the_review_head_for_reobservation() {
    let fixture = ConflictFixture::new("target.txt");

    let outcome = fixture
        .authority
        .materialize_merge_conflict(&fixture.request, GitHubCredential("test-token"))
        .await
        .assert_value();

    assert_reobservation_cleanup(&fixture, outcome, ExpectedCleanup::AbortedPendingMerge);
    assert!(!fixture.repository.workspace.join("target.txt").exists());
}

#[tokio::test]
async fn stale_conflict_observation_accepts_an_already_integrated_target() {
    let fixture = ConflictFixture::target_is_ancestor();

    let outcome = fixture
        .authority
        .materialize_merge_conflict(&fixture.request, GitHubCredential("test-token"))
        .await
        .assert_value();

    assert_reobservation_cleanup(&fixture, outcome, ExpectedCleanup::AlreadyClean);
}

fn assert_reobservation_cleanup(
    fixture: &ConflictFixture,
    outcome: GitHubConflictOutcome,
    expected: ExpectedCleanup,
) {
    assert_eq!(outcome, GitHubConflictOutcome::ObservationChanged);
    assert_eq!(
        git_output(&fixture.repository.workspace, &["rev-parse", "HEAD"]),
        fixture.request.review.head_revision
    );
    assert_eq!(
        git_output(
            &fixture.repository.workspace,
            &["status", "--porcelain=v1", "--untracked-files=all"]
        ),
        ""
    );
    assert!(
        !fixture
            .repository
            .workspace
            .join(".git/MERGE_HEAD")
            .exists()
    );
    let git_capture =
        fs::read_to_string(format!("{}.capture", fixture.git_program.display())).assert_value();
    match expected {
        ExpectedCleanup::AbortedPendingMerge => assert!(git_capture.contains("arg=--abort")),
        ExpectedCleanup::AlreadyClean => assert!(!git_capture.contains("arg=--abort")),
    }
    assert_transport_keeps_credential_ephemeral(fixture);
}

fn commit_all(workspace: &Path, message: &str) {
    git(workspace, &["add", "--all"]);
    git(
        workspace,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--no-verify",
            "--message",
            message,
        ],
    );
}

fn git_wrapper(repository: &TestGitRepository) -> PathBuf {
    let remote = path_text(&repository.remote);
    assert!(!remote.contains('\''));
    repository.root.write_executable(
        "git-wrapper",
        &format!(
            r#"#!/bin/bash
set -eu
capture="${{0}}.capture"
/usr/bin/printf 'token=%s\n' "${{GH_TOKEN-unset}}" >> "$capture"
for argument in "$@"; do /usr/bin/printf 'arg=%s\n' "$argument" >> "$capture"; done
arguments=()
for argument in "$@"; do
  if [[ "$argument" == 'https://github.com/acme/project.git' ]]; then
    arguments+=('{remote}')
  else
    arguments+=("$argument")
  fi
done
exec /usr/bin/git "${{arguments[@]}}"
"#,
        ),
    )
}

fn gh_script(repository: &TestGitRepository, target_revision: &str) -> PathBuf {
    repository.root.write_executable(
        "gh-target-ref",
        &format!(
            r#"#!/bin/sh
set -eu
/usr/bin/printf 'token=%s\n' "${{GH_TOKEN-unset}}" >> "${{0}}.capture"
for argument in "$@"; do /usr/bin/printf 'arg=%s\n' "$argument" >> "${{0}}.capture"; done
/usr/bin/printf '%s\n' '{{"ref":"refs/heads/main","object":{{"sha":"{target_revision}","type":"commit"}}}}'
"#,
        ),
    )
}

fn assert_transport_keeps_credential_ephemeral(fixture: &ConflictFixture) {
    let git_capture =
        fs::read_to_string(format!("{}.capture", fixture.git_program.display())).assert_value();
    assert!(git_capture.contains("token=test-token"));
    assert!(git_capture.contains("token=unset"));
    let merge_invocation = git_capture
        .split("token=")
        .find(|invocation| invocation.contains("arg=merge\n"))
        .assert_value();
    assert!(merge_invocation.starts_with("unset\n"));
    let fetch_invocation = git_capture
        .split("token=")
        .find(|invocation| invocation.contains("arg=fetch\n"))
        .assert_value();
    assert!(fetch_invocation.starts_with("test-token\n"));
    assert!(git_capture.contains(&format!("arg={}", fixture.target_revision)));
    assert!(!git_capture.lines().any(|line| line == "arg=test-token"));
    let gh_capture =
        fs::read_to_string(format!("{}.capture", fixture.gh_program.display())).assert_value();
    assert!(gh_capture.contains("token=test-token"));
    assert!(gh_capture.contains("arg=api"));
    assert!(gh_capture.contains("arg=repos/acme/project/git/ref/heads/main"));
    assert!(!gh_capture.lines().any(|line| line == "arg=test-token"));
    let config =
        fs::read_to_string(fixture.repository.workspace.join(".git/config")).assert_value();
    assert!(!config.contains("test-token"));
    assert!(!config.contains("extraheader"));
}
