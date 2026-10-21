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
fn git_metadata_commands_exclude_the_cloud_machine_credential() {
    let command = git_command(None);
    assert!(
        command
            .as_std()
            .get_envs()
            .any(|(name, value)| { name == "ZEROSHOT_API_KEY" && value.is_none() })
    );
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

#[tokio::test]
async fn named_source_uses_remote_upstream_tip_and_preserves_dirty_selected_workspace() {
    let source = initialized_repository("remote-tip-source");
    let remote_revision = commit_empty(&source.0, "remote source");
    output(&source.0, &["branch", "-M", "release"]);
    let client = initialized_repository("remote-tip-client");
    let local_revision = commit_empty(&client.0, "different local work");
    output(&client.0, &["branch", "-M", "feature"]);
    add_remote(
        &client.0,
        "origin",
        "https://forge.example/team/project.git",
    );
    output(
        &client.0,
        &[
            "fetch",
            source.0.to_str().assert_value(),
            "release:refs/remotes/origin/release",
        ],
    );
    output(
        &client.0,
        &["branch", "--set-upstream-to", "origin/release", "feature"],
    );
    let remote = "https://forge.example/team/project.git";
    configure_local_mirror(&client.0, remote, &source.0);
    let nested = client.0.join("nested");
    std::fs::create_dir(&nested).assert_value();
    std::fs::write(nested.join("caller-owned.txt"), "uncommitted work\n").assert_value();
    let config_before = std::fs::read(client.0.join(".git/config")).assert_value();
    let mut command = private_source_command(
        &nested,
        RunRepository::Git(GitRemoteUrl::new(remote).assert_value()),
    );
    command.branch = None;
    command.revision = None;
    command.source_connection = None;

    let selected = resolve(
        NamedSourceRequest {
            run: &command,
            token: None,
            provided: &RunConnectionValues::new(),
            required_capabilities: &[crate::source_code_provider::SourceCapability::Push],
        },
        &|name| panic!("public source must not request credential field {name}"),
    )
    .await
    .assert_value()
    .assert_value();

    assert_public_git_source(&selected, remote, "release", remote_revision.trim());
    assert!(selected.source.dirty);
    assert_eq!(output(&client.0, &["rev-parse", "HEAD"]), local_revision);
    assert_eq!(
        std::fs::read_to_string(nested.join("caller-owned.txt")).assert_value(),
        "uncommitted work\n"
    );
    assert_eq!(
        std::fs::read(client.0.join(".git/config")).assert_value(),
        config_before
    );
}

#[tokio::test]
async fn github_shorthand_fallback_resolves_remote_tip_and_distinguishes_missing_refs() {
    let source = initialized_repository("github-fallback-source");
    let revision = commit_empty(&source.0, "fallback source");
    output(&source.0, &["branch", "-M", "release"]);
    let client = initialized_repository("github-fallback-client");
    configure_local_mirror(&client.0, "https://github.com/owner/project.git", &source.0);
    let mut command = private_source_command(
        &client.0,
        RunRepository::GitHub(SourceRepositoryId::new("owner/project").assert_value()),
    );
    command.branch = Some(SourceBranchId::new("release").assert_value());
    command.revision = None;
    command.source_connection = None;
    let provided = RunConnectionValues::new();
    let request = |run| NamedSourceRequest {
        run,
        token: None,
        provided: &provided,
        required_capabilities: &[],
    };
    let unavailable = |name: &str| panic!("GitHub shorthand read source field {name}");

    let selected = resolve(request(&command), &unavailable)
        .await
        .assert_value()
        .assert_value();
    assert_eq!(
        selected.source.resolved.repository.as_str(),
        "owner/project"
    );
    assert_eq!(selected.source.resolved.revision.as_str(), revision.trim());
    assert!(selected.source.resolved.remote.is_none());
    assert!(selected.git_credentials.is_none());

    let mut missing_command = command.clone();
    missing_command.branch = Some(SourceBranchId::new("missing").assert_value());
    let missing = resolve(request(&missing_command), &unavailable)
        .await
        .err()
        .expect("an absent remote branch must not use local HEAD");
    assert!(
        missing
            .to_string()
            .contains("selected remote branch does not exist")
    );

    drop(source);
    let inaccessible = resolve(request(&missing_command), &unavailable)
        .await
        .err()
        .expect("an unavailable remote must not be mistaken for an absent ref");
    assert!(
        inaccessible
            .to_string()
            .contains("could not resolve the remote branch tip")
    );
}

#[tokio::test]
async fn detached_named_sources_require_both_repository_and_branch_before_credentials() {
    let root = initialized_repository("detached-source");
    commit_empty(&root.0, "detached source");
    output(&root.0, &["checkout", "--detach"]);
    let repository = RunRepository::Git(
        GitRemoteUrl::new("ssh://git@forge.example/team/project.git").assert_value(),
    );
    let mut command = private_source_command(&root.0, repository.clone());
    command.source_connection = None;
    let provided = RunConnectionValues::new();
    let unavailable = |name: &str| panic!("detached source read credential field {name}");
    for (repository, branch) in [
        (None, None),
        (Some(repository.clone()), None),
        (None, Some(SourceBranchId::new("main").assert_value())),
    ] {
        command.repository = repository;
        command.branch = branch;
        let error = resolve(
            NamedSourceRequest {
                run: &command,
                token: None,
                provided: &provided,
                required_capabilities: &[],
            },
            &unavailable,
        )
        .await
        .err()
        .expect("detached provenance must be explicit");
        assert!(
            error
                .to_string()
                .contains("detached worktrees require both")
        );
    }
    command.repository = Some(repository);
    command.branch = Some(SourceBranchId::new("main").assert_value());
    let selected = resolve(
        NamedSourceRequest {
            run: &command,
            token: None,
            provided: &provided,
            required_capabilities: &[],
        },
        &unavailable,
    )
    .await
    .assert_value()
    .assert_value();
    assert_eq!(
        Some(&selected.source.resolved.revision),
        command.revision.as_ref()
    );
    assert!(!selected.source.dirty);

    let plain = TestDirectory::new("plain-named-source");
    command.cwd = Some(plain.0.clone());
    assert!(
        inspect_worktree(&command)
            .await
            .err()
            .expect("named source inspection needs a Git worktree")
            .to_string()
            .contains("run from a Git worktree")
    );
    assert!(
        worktree_is_dirty(&plain.0)
            .await
            .assert_error()
            .to_string()
            .contains("could not inspect worktree status")
    );
}

#[tokio::test]
async fn repository_inference_ignores_ineligible_remotes_and_local_only_upstream() {
    let root = initialized_repository("ineligible-remotes");
    commit_empty(&root.0, "local upstream");
    let branch = output(&root.0, &["branch", "--show-current"]);
    output(
        &root.0,
        &["config", &format!("branch.{}.remote", branch.trim()), "."],
    );
    output(
        &root.0,
        &[
            "config",
            &format!("branch.{}.merge", branch.trim()),
            "refs/heads/local",
        ],
    );
    assert!(
        upstream(&root.0, branch.trim())
            .await
            .assert_value()
            .is_none()
    );
    add_remote(&root.0, "filesystem", root.0.to_str().assert_value());
    add_remote(&root.0, "unsupported", "http://forge.example/project.git");
    add_remote(
        &root.0,
        "credentialed",
        "https://user:private-value@forge.example/project.git",
    );
    let remote = "ssh://git@forge.example/nested/team/project.git";
    add_remote(&root.0, "eligible", remote);

    let (repository, selected) = select_repository(&root.0, Some("stale"))
        .await
        .assert_value();
    assert_eq!(repository.as_str(), remote);
    assert_eq!(selected.as_deref(), Some("eligible"));
    let (repository, selected) = select_repository(&root.0, Some("filesystem"))
        .await
        .assert_value();
    assert_eq!(repository.as_str(), remote);
    assert_eq!(selected.as_deref(), Some("eligible"));
    assert!(
        select_repository(&root.0.join("absent"), None)
            .await
            .assert_error()
            .to_string()
            .contains("could not list Git remotes")
    );
}

#[test]
fn long_remote_identities_remain_bounded_stable_and_distinct() {
    let first = format!("https://forge.example/{}/project.git", "nested/".repeat(50));
    let second = format!("https://forge.example/{}/other.git", "nested/".repeat(50));
    let identity = remote_repository(&first).assert_value();
    assert_eq!(identity, remote_repository(&first).assert_value());
    assert_ne!(identity, remote_repository(&second).assert_value());
    assert!(identity.as_str().len() <= 255);
    assert!(GitRemoteUrl::new(first).is_ok());
    assert!(
        remote_repository("https://user:private-value@forge.example/project.git")
            .assert_error()
            .to_string()
            .contains("credential-free HTTPS or SSH")
    );
}

#[tokio::test]
async fn github_remote_matching_preserves_legacy_and_explicit_source_authority() {
    let root = initialized_repository("github-source-authority");
    add_remote(&root.0, "a-invalid", root.0.to_str().assert_value());
    add_remote(&root.0, "b-unrelated", "https://github.com/owner/other.git");
    let remote_url = "git@github.com:owner/project.git";
    add_remote(&root.0, "c-source", remote_url);
    let mut command = private_source_command(
        &root.0,
        RunRepository::GitHub(SourceRepositoryId::new("owner/project").assert_value()),
    );
    let key = command.source_connection.clone().assert_value();
    command.source_connection = None;
    let provided = RunConnectionValues::new();
    let public = resolve(
        NamedSourceRequest {
            run: &command,
            token: None,
            provided: &provided,
            required_capabilities: &[crate::source_code_provider::SourceCapability::PullRequest],
        },
        &|name| panic!("legacy GitHub source read a transport field: {name}"),
    )
    .await
    .assert_value()
    .assert_value();
    assert_eq!(public.source.resolved.repository.as_str(), "owner/project");
    assert!(public.source.resolved.remote.is_none());
    assert!(public.git_credentials.is_none());

    let credentials = GitTransportCredentials::Ssh {
        private_key: "source-only-private-key".to_owned(),
        known_hosts: "source-only-known-hosts".to_owned(),
    };
    let provided =
        RunConnectionValues::from([(key.clone(), credentials.connection_values().assert_value())]);
    command.source_connection = Some(key.clone());
    let private = resolve(
        NamedSourceRequest {
            run: &command,
            token: None,
            provided: &provided,
            required_capabilities: &[crate::source_code_provider::SourceCapability::PullRequest],
        },
        &|name| panic!("captured SSH transport field was read again: {name}"),
    )
    .await
    .assert_value()
    .assert_value();
    let metadata = private.source.resolved.remote.as_ref().assert_value();
    assert_eq!(metadata.url.as_str(), remote_url);
    assert_eq!(metadata.connection.as_ref(), Some(&key));
    assert_eq!(private.git_credentials, Some(credentials));
    let provenance = serde_json::to_string(&private.source.resolved).assert_value();
    assert!(!provenance.contains("source-only-private-key"));
    assert!(!provenance.contains("source-only-known-hosts"));
}

#[test]
fn source_connections_scope_native_reads_and_prefer_captured_values() {
    use openengine_cluster_protocol::{ConnectionKey, EnvironmentVariableName, StaticConnectionValues};

    let key = ConnectionKey::new("source").assert_value();
    let values = StaticConnectionValues::new(std::collections::BTreeMap::from([
        (
            EnvironmentVariableName::new("GIT_USERNAME").assert_value(),
            "captured-user".to_owned(),
        ),
        (
            EnvironmentVariableName::new("GH_TOKEN").assert_value(),
            "unrelated-private-value".to_owned(),
        ),
    ]))
    .assert_value();
    let provided = RunConnectionValues::from([(key.clone(), values)]);
    let mut remote = ResolvedGitRemote {
        url: GitRemoteUrl::new("https://forge.example/project.git").assert_value(),
        forge: None,
        connection: Some(key),
    };
    let selected = source_credentials(Some(&remote), &provided, &|name| {
        assert_eq!(
            name, "GIT_PASSWORD",
            "only an uncaptured authorized field may be read"
        );
        Some(OsString::from("native-password"))
    })
    .assert_value();
    assert_eq!(
        selected,
        Some(GitTransportCredentials::Https {
            username: "captured-user".to_owned(),
            password: "native-password".to_owned(),
        })
    );
    let missing = source_credentials(Some(&remote), &provided, &|_| None).assert_error();
    assert!(
        missing
            .to_string()
            .contains("exactly one complete HTTPS or SSH credential shape")
    );
    assert!(!missing.to_string().contains("unrelated-private-value"));

    remote.url = GitRemoteUrl::new("git@forge.example:team/project.git").assert_value();
    assert_eq!(
        source_credentials(Some(&remote), &provided, &|name| match name {
            "GIT_SSH_PRIVATE_KEY" => Some(OsString::from("private-key")),
            "GIT_SSH_KNOWN_HOSTS" => Some(OsString::from("known-hosts")),
            _ => panic!("SSH source read an unauthorized field: {name}"),
        })
        .assert_value(),
        Some(GitTransportCredentials::Ssh {
            private_key: "private-key".to_owned(),
            known_hosts: "known-hosts".to_owned(),
        })
    );
    remote.connection = None;
    let unavailable = |name: &str| panic!("source without a connection read {name}");
    assert!(
        source_credentials(Some(&remote), &provided, &unavailable)
            .assert_value()
            .is_none()
    );
    assert!(
        source_credentials(None, &provided, &unavailable)
            .assert_value()
            .is_none()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn inferred_named_remote_resolves_local_branch_through_native_ssh_configuration() {
    let source = initialized_repository("native-ssh-source");
    let revision = commit_empty(&source.0, "native SSH source");
    output(&source.0, &["branch", "-M", "release"]);
    let client = initialized_repository("native-ssh-client");
    commit_empty(&client.0, "different local SSH client");
    output(&client.0, &["branch", "-M", "release"]);
    let remote = "ssh://git@forge.example/team/project.git";
    add_remote(&client.0, "origin", remote);
    let helper = client.0.join(".git/native-ssh");
    let quoted_source = source.0.to_string_lossy().replace('\'', "'\\''");
    openengine_cluster_testkit::fixture::write_executable(
        &helper,
        format!("#!/bin/sh\nexec git upload-pack '{quoted_source}'\n"),
        0o700,
    )
    .assert_value();
    let quoted_helper = helper.to_string_lossy().replace('\'', "'\\''");
    output(
        &client.0,
        &[
            "config",
            "core.sshCommand",
            &format!("sh '{quoted_helper}'"),
        ],
    );
    output(&client.0, &["config", "ssh.variant", "simple"]);
    let mut command = test_run_command();
    command.target = Some("prod".to_owned());
    command.cwd = Some(client.0.clone());

    let selected = resolve(
        NamedSourceRequest {
            run: &command,
            token: None,
            provided: &RunConnectionValues::new(),
            required_capabilities: &[],
        },
        &|name| panic!("native SSH configuration requested an undeclared source field: {name}"),
    )
    .await
    .assert_value()
    .assert_value();

    assert_public_git_source(&selected, remote, "release", revision.trim());
    assert!(!selected.source.dirty);
}

#[cfg(unix)]
#[tokio::test]
async fn git_metadata_rejects_failed_non_utf8_and_oversized_command_output() {
    for script in [
        "printf ignored; exit 1",
        "printf '\\377'",
        "head -c 1048577 /dev/zero",
    ] {
        assert!(
            super::run(Command::new("sh").args(["-c", script]))
                .await
                .is_err()
        );
    }
}

fn commit_empty(root: &Path, message: &str) -> String {
    output(
        root,
        &[
            "-c",
            "user.name=Zeroshot Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            message,
        ],
    );
    output(root, &["rev-parse", "HEAD"])
}

fn configure_local_mirror(root: &Path, remote: &str, mirror: &Path) {
    let mirror = url::Url::from_directory_path(mirror).expect("absolute mirror directory");
    output(
        root,
        &["config", &format!("url.{mirror}.insteadOf"), remote],
    );
}

fn assert_public_git_source(
    selected: &NamedSourceResolution,
    remote: &str,
    branch: &str,
    revision: &str,
) {
    let source = &selected.source.resolved;
    assert_eq!(source.repository.as_str(), remote);
    assert_eq!(source.branch.as_str(), branch);
    assert_eq!(source.revision.as_str(), revision);
    let metadata = source.remote.as_ref().assert_value();
    assert_eq!(metadata.url.as_str(), remote);
    assert!(metadata.forge.is_none());
    assert!(metadata.connection.is_none());
    assert!(selected.git_credentials.is_none());
}
