use super::*;
use openengine_cluster_protocol::RunTitle;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use crate::native_v2_cli::RunSelection;

struct TestDirectory(std::path::PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "zeroshot-named-source-{label}-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn parses_supported_github_remote_urls() {
    for url in [
        "git@github.com:owner/repo.git",
        "ssh://git@github.com/owner/repo.git",
        "https://github.com/owner/repo.git",
    ] {
        assert_eq!(
            parse_github_repository(url)
                .map(|value| value.to_string())
                .as_deref(),
            Some("owner/repo")
        );
    }
    assert!(parse_github_repository("https://example.com/owner/repo.git").is_none());
}

#[tokio::test]
async fn explicit_repository_reuses_authenticated_ssh_remote() {
    let root = TestDirectory::new("ssh");
    run(std::process::Command::new("git").arg("init").arg(&root.0)).unwrap();
    run(std::process::Command::new("git")
        .current_dir(&root.0)
        .args([
            "remote",
            "add",
            "private",
            "git@github.com:owner/private.git",
        ]))
    .unwrap();

    let repository = SourceRepositoryId::new("owner/private").unwrap();
    assert_eq!(
        matching_remote(&root.0, &repository, None).await.as_deref(),
        Some("private")
    );
    assert_eq!(
        matching_remote(&root.0, &repository, Some("private"))
            .await
            .as_deref(),
        Some("private")
    );
}

#[tokio::test]
async fn github_upstream_takes_precedence_over_other_github_remotes() {
    let root = initialized_repository("github-upstream");
    add_remote(&root.0, "upstream", "git@github.com:owner/upstream.git");
    add_remote(&root.0, "fork", "https://github.com/owner/fork.git");

    let (repository, remote) = select_repository(&root.0, Some("upstream")).await.unwrap();

    assert_eq!(repository.as_str(), "owner/upstream");
    assert_eq!(remote.as_deref(), Some("upstream"));
}

#[tokio::test]
async fn non_github_or_stale_upstream_falls_back_to_the_only_github_remote() {
    let root = initialized_repository("non-github-upstream");
    add_remote(&root.0, "mirror", "https://gitlab.com/owner/project.git");
    add_remote(&root.0, "github", "https://github.com/owner/project.git");

    for preferred in ["mirror", "stale"] {
        let (repository, remote) = select_repository(&root.0, Some(preferred)).await.unwrap();

        assert_eq!(repository.as_str(), "owner/project");
        assert_eq!(remote.as_deref(), Some("github"));
    }
}

#[tokio::test]
async fn repository_inference_requires_one_github_remote_and_ignores_missing_upstream() {
    let root = initialized_repository("repository-cardinality");
    let branch = output(&root.0, &["branch", "--show-current"]);
    assert!(
        upstream(&root.0, branch.trim())
            .await
            .assert_value()
            .is_none()
    );

    let missing = select_repository(&root.0, None).await.assert_error();
    assert!(missing.to_string().contains("no GitHub remote"));

    add_remote(&root.0, "first", "https://github.com/owner/first.git");
    add_remote(&root.0, "second", "git@github.com:owner/second.git");
    let ambiguous = select_repository(&root.0, None).await.assert_error();
    assert!(ambiguous.to_string().contains("multiple GitHub remotes"));
}

#[tokio::test]
async fn dirty_status_includes_untracked_unstaged_and_staged_changes() {
    let root = initialized_repository("dirty");
    let tracked = root.0.join("tracked.txt");
    std::fs::write(&tracked, "clean\n").unwrap();
    run(std::process::Command::new("git")
        .current_dir(&root.0)
        .args(["add", "tracked.txt"]))
    .unwrap();
    run(std::process::Command::new("git")
        .current_dir(&root.0)
        .args([
            "-c",
            "user.name=Zeroshot Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ]))
    .unwrap();
    assert!(!worktree_is_dirty(&root.0).await.unwrap());

    std::fs::write(root.0.join("untracked.txt"), "new\n").unwrap();
    assert!(worktree_is_dirty(&root.0).await.unwrap());
    std::fs::remove_file(root.0.join("untracked.txt")).unwrap();

    std::fs::write(&tracked, "changed\n").unwrap();
    assert!(worktree_is_dirty(&root.0).await.unwrap());

    run(std::process::Command::new("git")
        .current_dir(&root.0)
        .args(["add", "tracked.txt"]))
    .unwrap();
    assert!(worktree_is_dirty(&root.0).await.unwrap());
}

#[tokio::test]
async fn git_commands_preserve_https_credential_helper_configuration() {
    let home = TestDirectory::new("credential-helper");
    std::fs::write(
        home.0.join(".gitconfig"),
        "[credential]\n\thelper = private-helper\n",
    )
    .unwrap();
    let mut command = git_command(None);
    command
        .env("HOME", &home.0)
        .args(["config", "--global", "--get", "credential.helper"]);

    assert_eq!(
        super::run(&mut command).await.unwrap().trim(),
        "private-helper"
    );
}

#[tokio::test]
async fn wave6_cli_contract_source_selection_obeys_explicit_upstream_and_local_precedence() {
    let root = initialized_repository("selection");
    let mut command = test_run_command();
    assert!(resolve(&command, None).await.assert_value().is_none());
    command.target = Some("prod".to_owned());
    command.validate_only = true;
    assert!(resolve(&command, None).await.assert_value().is_none());
    command.validate_only = false;

    let upstream = SourceBranchId::new("upstream-main").assert_value();
    let worktree = WorktreeSourceContext {
        root: root.0.clone(),
        local_branch: Some("local-main".to_owned()),
        upstream: Some(("origin".to_owned(), upstream.clone())),
    };
    assert_eq!(
        select_run_branch(&command, &worktree).assert_value(),
        upstream
    );

    command.branch = Some(SourceBranchId::new("explicit-main").assert_value());
    assert_eq!(
        select_run_branch(&command, &worktree)
            .assert_value()
            .as_str(),
        "explicit-main"
    );
    command.branch = None;
    let local_only = WorktreeSourceContext {
        root: root.0.clone(),
        local_branch: Some("local-main".to_owned()),
        upstream: None,
    };
    assert_eq!(
        select_run_branch(&command, &local_only)
            .assert_value()
            .as_str(),
        "local-main"
    );
    let unavailable = WorktreeSourceContext {
        root: root.0.clone(),
        local_branch: None,
        upstream: None,
    };
    assert!(
        select_run_branch(&command, &unavailable)
            .assert_error()
            .to_string()
            .contains("supply --branch")
    );
}

#[tokio::test]
async fn wave6_cli_contract_remote_tip_is_exact_and_missing_branches_fail_closed() {
    let source = initialized_repository("remote-source");
    std::fs::write(source.0.join("tracked.txt"), "source\n").assert_value();
    run(std::process::Command::new("git")
        .current_dir(&source.0)
        .args(["add", "tracked.txt"]))
    .assert_value();
    run(std::process::Command::new("git")
        .current_dir(&source.0)
        .args([
            "-c",
            "user.name=Zeroshot Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "source",
        ]))
    .assert_value();
    let revision = output(&source.0, &["rev-parse", "HEAD"]);
    let branch = output(&source.0, &["branch", "--show-current"]);

    let client = initialized_repository("remote-client");
    add_remote(&client.0, "origin", source.0.to_str().assert_value());
    let repository = SourceRepositoryId::new("owner/repository").assert_value();
    let branch = SourceBranchId::new(branch.trim()).assert_value();
    let selection = || RevisionSelection {
        root: &client.0,
        repository: &repository,
        branch: &branch,
        remote: Some("origin"),
        token: None,
    };
    assert_eq!(
        remote_tip(selection()).await.assert_value().as_str(),
        revision.trim()
    );

    let missing = SourceBranchId::new("missing").assert_value();
    assert!(
        remote_tip(RevisionSelection {
            root: &client.0,
            repository: &repository,
            branch: &missing,
            remote: Some("origin"),
            token: None,
        })
        .await
        .is_err()
    );

    let explicit = SourceRevisionId::new(revision.trim()).assert_value();
    let mut command = test_run_command();
    command.revision = Some(explicit.clone());
    assert_eq!(
        select_run_revision(&command, selection())
            .await
            .assert_value(),
        explicit
    );
}

fn test_run_command() -> RunCommand {
    RunCommand {
        target: None,
        title: RunTitle::new("Source selection").assert_value(),
        input: "unused.json".into(),
        selection: RunSelection::Profile(None),
        repository: None,
        branch: None,
        revision: None,
        detach: true,
        validate_only: false,
        submission_key: None,
    }
}

fn output(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .assert_value();
    assert!(output.status.success());
    String::from_utf8(output.stdout).assert_value()
}

fn initialized_repository(label: &str) -> TestDirectory {
    let root = TestDirectory::new(label);
    run(std::process::Command::new("git").arg("init").arg(&root.0)).unwrap();
    root
}

fn add_remote(root: &Path, name: &str, url: &str) {
    run(std::process::Command::new("git")
        .current_dir(root)
        .args(["remote", "add", name, url]))
    .unwrap();
}

fn run(command: &mut std::process::Command) -> std::io::Result<()> {
    let status = command.status()?;
    assert!(status.success());
    Ok(())
}
