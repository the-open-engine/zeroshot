use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::native_v2_candidate::test_support::{TestGitRepository, git_output};

#[tokio::test]
async fn uses_manifest_title_as_the_commit_message() {
    let repository = TestGitRepository::delivery();
    let git = SystemGit::new(PathBuf::from("git"));

    let revision = git
        .prepare_revision(
            &repository.workspace,
            &repository.base,
            "fix: repair checkout",
        )
        .await
        .assert_value();

    assert_eq!(
        git_output(
            &repository.workspace,
            &["show", "--no-patch", "--format=%s", &revision]
        ),
        "fix: repair checkout"
    );
}
#[tokio::test]
async fn unresolved_index_is_preserved_until_the_caller_resolves_it() {
    for resolution in ["combined change\n", "local change\n"] {
        assert_conflict_resolution(resolution).await;
    }
}

async fn assert_conflict_resolution(resolution: &str) {
    use crate::native_v2_candidate::test_support::git;
    let repository = TestGitRepository::candidate();
    let workspace = &repository.workspace;
    git(workspace, &["config", "user.name", "Test"]);
    git(workspace, &["config", "user.email", "test@example.invalid"]);
    git(workspace, &["checkout", "-b", "other"]);
    std::fs::write(workspace.join("README.md"), "remote change\n").assert_value();
    git(workspace, &["commit", "-am", "remote change"]);
    git(workspace, &["checkout", "main"]);
    std::fs::write(workspace.join("README.md"), "local change\n").assert_value();
    git(workspace, &["commit", "-am", "local change"]);
    let head = git_output(workspace, &["rev-parse", "HEAD"]);
    let merge = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["merge", "other"])
        .output()
        .assert_value();
    assert!(!merge.status.success());
    let index = git_output(workspace, &["ls-files", "--unmerged"]);
    assert!(!index.is_empty());
    let system = SystemGit::new(PathBuf::from("git"));
    let error = system
        .prepare_revision(workspace, &repository.base, "delivery")
        .await
        .unwrap_err();
    assert!(matches!(&error, GitError::Unmerged(_)));
    assert!(error.to_string().contains("README.md"));
    assert_eq!(git_output(workspace, &["ls-files", "--unmerged"]), index);
    assert_eq!(git_output(workspace, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        system.workspace_state(workspace).await.assert_value(),
        (head, true)
    );
    std::fs::write(workspace.join("README.md"), resolution).assert_value();
    git(workspace, &["add", "README.md"]);
    assert!(system.workspace_state(workspace).await.assert_value().1);
    let delivered = system
        .prepare_revision(workspace, &repository.base, "resolved")
        .await
        .assert_value();
    assert_eq!(
        system.workspace_state(workspace).await.assert_value(),
        (delivered, false)
    );
    assert!(git_output(workspace, &["ls-files", "--unmerged"]).is_empty());
    assert_eq!(
        git_output(workspace, &["show", "HEAD:README.md"]),
        resolution.trim()
    );
    assert_eq!(
        git_output(workspace, &["rev-list", "--parents", "-n", "1", "HEAD"])
            .split_whitespace()
            .count(),
        3
    );
}
