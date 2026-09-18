use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;
use crate::native_v2_candidate::test_support::{TestGitRepository, commit_all, git, git_output};

#[derive(Clone, Copy)]
enum Operation {
    RebaseMerge,
    RebaseApply,
    CherryPick,
    Revert,
    Sequencer,
}

impl Operation {
    fn arguments(self) -> &'static [&'static str] {
        match self {
            Self::RebaseMerge => &["rebase", "--merge", "remote"],
            Self::RebaseApply => &["rebase", "--apply", "remote"],
            Self::CherryPick => &["cherry-pick", "remote-first"],
            Self::Revert => &["revert", "--no-edit", "remote-first"],
            Self::Sequencer => &["cherry-pick", "remote-first", "remote-second"],
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::RebaseMerge => "rebase-merge",
            Self::RebaseApply => "rebase-apply",
            Self::CherryPick => "CHERRY_PICK_HEAD",
            Self::Revert => "REVERT_HEAD",
            Self::Sequencer => "sequencer",
        }
    }
}

#[tokio::test]
async fn unfinished_operations_preserve_staged_resolutions_in_repositories_and_worktrees() {
    for linked in [false, true] {
        for operation in [
            Operation::RebaseMerge,
            Operation::RebaseApply,
            Operation::CherryPick,
            Operation::Revert,
            Operation::Sequencer,
        ] {
            assert_operation_preserved(operation, linked).await;
        }
    }
}

async fn assert_operation_preserved(operation: Operation, linked: bool) {
    let repository = conflicting_history();
    let workspace = operation_workspace(&repository, linked);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&workspace)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(operation.arguments())
        .output()
        .assert_value();
    assert!(!output.status.success());
    assert!(!git_output(&workspace, &["ls-files", "--unmerged"]).is_empty());
    std::fs::write(workspace.join("README.md"), "resolved work\n").assert_value();
    git(&workspace, &["add", "README.md"]);
    if matches!(operation, Operation::Sequencer) {
        commit_all(&workspace, "resolved first change");
    }
    let directory = PathBuf::from(git_output(&workspace, &["rev-parse", "--absolute-git-dir"]));
    assert!(directory.join(operation.marker()).exists());
    if matches!(operation, Operation::Sequencer) {
        assert!(
            git_output(
                &workspace,
                &["--no-optional-locks", "status", "--porcelain=v1"]
            )
            .is_empty()
        );
    }
    let head = git_output(&workspace, &["rev-parse", "HEAD"]);
    let index = std::fs::read(directory.join("index")).assert_value();
    let system = SystemGit::new(PathBuf::from("git"));
    let state_error = system
        .workspace_state(&workspace)
        .await
        .assert_error()
        .to_string();
    assert!(state_error.contains(operation.marker()), "{state_error}");
    std::fs::write(workspace.join("untracked.txt"), "keep untracked\n").assert_value();
    let error = system
        .prepare_revision(&workspace, &repository.base, "delivery")
        .await
        .assert_error()
        .to_string();
    assert!(error.contains(operation.marker()), "{error}");
    assert!(
        error.contains("no files were staged or committed"),
        "{error}"
    );
    assert!(error.contains("status"), "{error}");
    assert!(error.contains("untracked.txt"), "{error}");
    assert_eq!(git_output(&workspace, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read(directory.join("index")).assert_value(), index);
    assert!(directory.join(operation.marker()).exists());
    assert_eq!(
        std::fs::read_to_string(workspace.join("README.md")).assert_value(),
        "resolved work\n"
    );
    git(&workspace, &[operation.arguments()[0], "--abort"]);
    assert!(!directory.join(operation.marker()).exists());
    system
        .prepare_revision(&workspace, &repository.base, "delivery after agent abort")
        .await
        .assert_value();
}

fn conflicting_history() -> TestGitRepository {
    let repository = TestGitRepository::candidate();
    let workspace = &repository.workspace;
    git(workspace, &["checkout", "-b", "remote"]);
    std::fs::write(workspace.join("README.md"), "remote change\n").assert_value();
    commit_all(workspace, "remote first");
    git(workspace, &["tag", "remote-first"]);
    std::fs::write(workspace.join("later.txt"), "later change\n").assert_value();
    commit_all(workspace, "remote second");
    git(workspace, &["tag", "remote-second"]);
    git(workspace, &["checkout", "main"]);
    std::fs::write(workspace.join("README.md"), "local change\n").assert_value();
    commit_all(workspace, "local change");
    repository
}

fn operation_workspace(repository: &TestGitRepository, linked: bool) -> PathBuf {
    if !linked {
        return repository.workspace.clone();
    }
    let workspace = repository.root.child("linked-worktree");
    git(
        &repository.workspace,
        &[
            "worktree",
            "add",
            "-b",
            "repair",
            workspace.to_str().assert_value(),
        ],
    );
    assert!(workspace.join(".git").is_file());
    workspace
}
