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
fn registered_github_identity_recognizes_supported_remote_forms() {
    for url in [
        "git@github.com:owner/repo.git",
        "ssh://git@github.com/owner/repo.git",
        "https://github.com/owner/repo.git",
    ] {
        let remote = GitRemoteUrl::new(url).assert_value();
        let identity = crate::source_code_provider::adapters::core::forge_identity(&remote)
            .assert_value()
            .assert_value();
        assert_eq!(identity.provider.as_str(), "source.github");
        assert_eq!(identity.repository.as_str(), "owner/repo");
    }
    let unrelated = GitRemoteUrl::new("https://example.com/owner/repo.git").assert_value();
    assert!(
        crate::source_code_provider::adapters::core::forge_identity(&unrelated)
            .assert_value()
            .is_none()
    );
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
        matching_github_remote(&root.0, &repository, None)
            .await
            .as_deref(),
        Some("private")
    );
    assert_eq!(
        matching_github_remote(&root.0, &repository, Some("private"))
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
async fn eligible_upstream_takes_precedence_and_stale_upstream_is_ambiguous() {
    let root = initialized_repository("non-github-upstream");
    add_remote(&root.0, "mirror", "https://gitlab.com/owner/project.git");
    add_remote(&root.0, "github", "https://github.com/owner/project.git");

    let (repository, remote) = select_repository(&root.0, Some("mirror")).await.unwrap();
    assert_eq!(repository.as_str(), "https://gitlab.com/owner/project.git");
    assert_eq!(remote.as_deref(), Some("mirror"));
    assert!(select_repository(&root.0, Some("stale")).await.is_err());
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
    assert!(
        upstream(&root.0.join("missing-worktree"), branch.trim())
            .await
            .assert_error()
            .to_string()
            .contains("could not inspect the attached branch upstream")
    );

    let missing = select_repository(&root.0, None).await.assert_error();
    assert!(
        missing
            .to_string()
            .contains("no eligible HTTPS or SSH Git remote")
    );

    add_remote(&root.0, "first", "https://github.com/owner/first.git");
    add_remote(&root.0, "second", "git@github.com:owner/second.git");
    let ambiguous = select_repository(&root.0, None).await.assert_error();
    assert!(
        ambiguous
            .to_string()
            .contains("multiple eligible Git remotes")
    );
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
    for (target, validate_only) in [(None, false), (Some("prod".to_owned()), true)] {
        command.target = target;
        command.validate_only = validate_only;
        let selected = resolve(
            NamedSourceRequest {
                run: &command,
                token: None,
                provided: &RunConnectionValues::new(),
                required_capabilities: &[],
            },
            &|_| None,
        )
        .await
        .assert_value();
        assert!(selected.is_none());
    }
    command.target = Some("prod".to_owned());
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
async fn named_remote_tip_rejects_filesystem_and_missing_remotes() {
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
        git_credentials: None,
    };
    assert!(remote_tip(selection()).await.is_err());
    assert!(!revision.trim().is_empty());

    let missing = SourceBranchId::new("missing").assert_value();
    assert!(
        remote_tip(RevisionSelection {
            root: &client.0,
            repository: &repository,
            branch: &missing,
            remote: Some("origin"),
            token: None,
            git_credentials: None,
        })
        .await
        .is_err()
    );
    assert!(
        remote_tip(RevisionSelection {
            root: &client.0,
            repository: &repository,
            branch: &branch,
            remote: Some("missing-remote"),
            token: None,
            git_credentials: None,
        })
        .await
        .assert_error()
        .to_string()
        .contains("could not read the selected Git remote")
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
        environment: None,
        target: None,
        cwd: None,
        source_connection: None,
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

#[test]
fn source_credentials_reuse_captured_authorized_fields_without_reading_them_again() {
    let key = openengine_cluster_protocol::ConnectionKey::new("shared").assert_value();
    let credentials = GitTransportCredentials::Https {
        username: "user".to_owned(),
        password: "captured-secret".to_owned(),
    };
    let provided =
        RunConnectionValues::from([(key.clone(), credentials.connection_values().assert_value())]);
    let remote = ResolvedGitRemote {
        url: GitRemoteUrl::new("https://forge.example/group/project.git").assert_value(),
        forge: None,
        connection: Some(key),
    };
    let captured = source_credentials(Some(&remote), &provided, &|name| {
        panic!("captured source field was read again: {name}")
    })
    .assert_value();
    assert_eq!(captured, Some(credentials));
}

#[tokio::test]
async fn explicit_github_shorthand_retains_source_connection_without_matching_local_remote() {
    let root = initialized_repository("explicit-source-connection");
    add_remote(
        &root.0,
        "unrelated",
        "https://forge.example/team/unrelated.git",
    );
    let command = private_source_command(
        &root.0,
        RunRepository::GitHub(SourceRepositoryId::new("owner/private").assert_value()),
    );
    let key = command.source_connection.clone().assert_value();
    let credentials = GitTransportCredentials::Https {
        username: "private-source-user".to_owned(),
        password: "private-source-password".to_owned(),
    };
    let provided =
        RunConnectionValues::from([(key.clone(), credentials.connection_values().assert_value())]);

    let resolved = resolve(
        NamedSourceRequest {
            run: &command,
            token: None,
            provided: &provided,
            required_capabilities: &[crate::source_code_provider::SourceCapability::Push],
        },
        &|name| panic!("source credentials already supplied; unexpected read of {name}"),
    )
    .await
    .assert_value()
    .assert_value();
    let source = resolved.source.resolved;
    let remote = source.remote.as_ref().assert_value();
    assert_eq!(source.repository.as_str(), "owner/private");
    assert_eq!(source.branch.as_str(), "main");
    assert_eq!(source.revision, command.revision.assert_value());
    assert_eq!(remote.url.as_str(), "https://github.com/owner/private.git");
    assert_eq!(remote.connection.as_ref(), Some(&key));
    let forge = remote.forge.as_ref().assert_value();
    assert_eq!(forge.provider.as_str(), "source.github");
    assert_eq!(forge.repository.as_str(), "owner/private");
    assert_eq!(resolved.git_credentials, Some(credentials));
    let public_source = serde_json::to_string(&source).assert_value();
    assert!(!public_source.contains("private-source-user"));
    assert!(!public_source.contains("private-source-password"));
}

#[tokio::test]
async fn unsupported_forge_capability_is_rejected_before_source_credential_reads() {
    use crate::source_code_provider::SourceCapability;

    let root = initialized_repository("unsupported-source-capability");
    let command = private_source_command(
        &root.0,
        RunRepository::Git(
            GitRemoteUrl::new("https://forge.example/nested/team/project.git").assert_value(),
        ),
    );
    let provided = RunConnectionValues::new();
    let reads = std::cell::Cell::new(0);
    let available = |_: &str| {
        reads.set(reads.get() + 1);
        Some(OsString::from("source-value-must-remain-unread"))
    };

    for capability in [SourceCapability::PullRequest, SourceCapability::Merge] {
        let error = resolve(
            NamedSourceRequest {
                run: &command,
                token: None,
                provided: &provided,
                required_capabilities: &[capability],
            },
            &available,
        )
        .await
        .err()
        .expect("an unregistered forge must reject PR and merge delivery");
        assert!(
            error
                .to_string()
                .contains("PR and merge delivery require a registered forge identity")
        );
        assert_eq!(reads.get(), 0, "unsupported forge read source credentials");
        assert!(
            !error
                .to_string()
                .contains("source-value-must-remain-unread")
        );
    }
}

fn private_source_command(root: &Path, repository: RunRepository) -> RunCommand {
    let mut command = test_run_command();
    command.target = Some("prod".to_owned());
    command.cwd = Some(root.to_owned());
    command.repository = Some(repository);
    command.branch = Some(SourceBranchId::new("main").assert_value());
    command.revision = Some(SourceRevisionId::new("a".repeat(40)).assert_value());
    command.source_connection =
        Some(openengine_cluster_protocol::ConnectionKey::new("git-access").assert_value());
    command
}

#[tokio::test]
async fn ordinary_named_source_discovers_registered_forges_before_secrets_or_network() {
    use crate::source_code_provider::adapters::contributed::registration_tests::{
        HTTPS_REMOTE, SSH_REMOTE, assert_source,
    };
    use crate::source_code_provider::SourceCapability;
    let root = initialized_repository("contribution-discovery");
    add_remote(&root.0, "origin", HTTPS_REMOTE);
    for remote in [HTTPS_REMOTE, SSH_REMOTE] {
        run(std::process::Command::new("git")
            .current_dir(&root.0)
            .args(["remote", "set-url", "origin", remote]))
        .assert_value();
        for explicit in [false, true] {
            let mut command = private_source_command(
                &root.0,
                RunRepository::Git(GitRemoteUrl::new(remote).assert_value()),
            );
            command.source_connection = None;
            if !explicit {
                command.repository = None;
            }
            let provided = RunConnectionValues::new();
            let selected = resolve(
                NamedSourceRequest {
                    run: &command,
                    token: None,
                    provided: &provided,
                    required_capabilities: &[
                        SourceCapability::PullRequest,
                        SourceCapability::Checks,
                    ],
                },
                &|name| panic!("discovery attempted an unauthorized credential read: {name}"),
            )
            .await
            .assert_value()
            .assert_value();
            assert_source(&selected.source.resolved, remote);
            assert_eq!(
                selected.source.resolved.revision,
                command.revision.assert_value()
            );
            assert!(selected.git_credentials.is_none());
        }
    }
}

#[tokio::test]
async fn ambiguous_registered_forge_stops_source_selection_before_credentials() {
    use crate::source_code_provider::adapters::contributed::registration_tests::{
        AMBIGUOUS_REMOTE, HTTPS_REMOTE,
    };
    use crate::source_code_provider::SourceCapability;
    let root = initialized_repository("ambiguous-contribution");
    add_remote(&root.0, "origin", AMBIGUOUS_REMOTE);
    add_remote(&root.0, "alternate", HTTPS_REMOTE);
    for preferred in [None, Some("origin")] {
        let error = select_repository(&root.0, preferred).await.assert_error();
        assert!(
            error
                .to_string()
                .contains("multiple source providers recognize")
        );
    }
    let provided = RunConnectionValues::new();
    for explicit in [false, true] {
        let mut command = private_source_command(
            &root.0,
            RunRepository::Git(GitRemoteUrl::new(AMBIGUOUS_REMOTE).assert_value()),
        );
        if !explicit {
            command.repository = None;
        }
        let error = resolve(
            NamedSourceRequest {
                run: &command,
                token: None,
                provided: &provided,
                required_capabilities: &[SourceCapability::PullRequest],
            },
            &|name| panic!("ambiguous discovery read source credentials: {name}"),
        )
        .await
        .err()
        .expect("ambiguous registered forge must stop source selection");
        assert!(
            error
                .to_string()
                .contains("multiple source providers recognize")
        );
    }
}
