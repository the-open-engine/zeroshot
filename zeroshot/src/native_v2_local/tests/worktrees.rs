use super::*;
use crate::native_v2_candidate::test_support::{TestGitRepository, git_output};
use crate::native_v2_cli::LocalWorktreeBase;
use crate::native_v2_local::workspace::{attach_run_branch, create_run_worktree, prepare_branch_source};

fn prepared(
    repo: &TestGitRepository,
    mode: Option<crate::native_v2_delivery::DeliveryMode>,
) -> PreparedLocalRun {
    let mut request = local_request(RunId::new("local-worktree-contract"));
    request.source = Some(crate::native_v2_cli::NamedRunSource {
        resolved: serde_json::from_value(json!({
            "repository":"open-engine/zeroshot", "branch":"main", "revision":repo.base,
            "remote":{"url":"https://github.com/open-engine/zeroshot.git", "forge":{"provider":"source.github","baseUrl":"https://github.com","repository":"open-engine/zeroshot"}}
        }))
        .assert_value(),
        dirty: false,
    });
    if let Some(mode) = mode {
        let mut node = crate::native_v2_candidate::test_support::git_delivery_node();
        node["worker"] = json!(mode.worker_ref());
        request.intent.graph = full_graph(vec![node, success_node()]);
    }
    prepare_local_run(request, &repo.workspace, Path::new("git")).assert_value()
}

fn redirect_remote(repo: &TestGitRepository) {
    git(
        &repo.workspace,
        &[
            "config",
            &format!("url.{}.insteadOf", repo.remote.display()),
            "https://github.com/open-engine/zeroshot.git",
        ],
    );
}

fn commit(repo: &TestGitRepository, message: &str) -> String {
    git(
        &repo.workspace,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-am",
            message,
        ],
    );
    git_output(&repo.workspace, &["rev-parse", "HEAD"])
}

#[tokio::test]
async fn local_and_remote_worktree_bases_preserve_the_invoking_checkout() {
    for base in [LocalWorktreeBase::Local, LocalWorktreeBase::Remote] {
        let repo = TestGitRepository::candidate();
        git(
            &repo.workspace,
            &["checkout", "-b", "feature/local", "--track", "origin/main"],
        );
        std::fs::write(repo.workspace.join("README.md"), "committed but unpushed\n").assert_value();
        let local = commit(&repo, "local commit");
        let mut prepared = prepared(&repo, None);
        prepared.submission.source.as_mut().unwrap().revision =
            openengine_cluster_protocol::SourceRevisionId::new(local.clone()).assert_value();
        redirect_remote(&repo);
        // Dirty/untracked resources are allowed without delivery and must never enter the new worktree.
        std::fs::write(repo.workspace.join(".gitignore"), "node_modules/\n.env\n").assert_value();
        std::fs::create_dir(repo.workspace.join("node_modules")).assert_value();
        std::fs::write(repo.workspace.join(".env"), "private resource").assert_value();
        std::fs::write(repo.workspace.join("untracked"), "resource").assert_value();
        git(&repo.workspace, &["add", ".gitignore"]);
        let index = git_output(&repo.workspace, &["write-tree"]);
        let status = git_output(&repo.workspace, &["status", "--porcelain=v1"]);
        let state = repo.root.child("state");
        prepare_branch_source(&mut prepared, Some(base), Path::new("git"), &state)
            .await
            .assert_value();
        let expected = if base == LocalWorktreeBase::Local {
            &local
        } else {
            &repo.base
        };
        assert_eq!(
            prepared
                .submission
                .source
                .as_ref()
                .unwrap()
                .revision
                .as_str(),
            expected
        );
        create_run_worktree(&mut prepared, Path::new("git"), &state)
            .await
            .assert_value();
        assert_ne!(prepared.workspace, repo.workspace);
        let branch = prepared.local_branch.as_ref().unwrap();
        assert!(branch.starts_with("zeroshot/v2-"));
        assert_eq!(
            git_output(&prepared.workspace, &["rev-parse", "HEAD"]),
            *expected
        );
        assert_eq!(
            git_output(&prepared.workspace, &["symbolic-ref", "--short", "HEAD"]),
            *branch
        );
        for name in ["node_modules", ".env", "untracked", ".gitignore"] {
            assert!(!prepared.workspace.join(name).exists());
        }
        std::fs::write(prepared.workspace.join("README.md"), "run change\n").assert_value();
        git(
            &prepared.workspace,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-am",
                "run change",
            ],
        );
        let transport = crate::native_v2_delivery::transport::GitTransport::new(
            crate::native_v2_delivery::transport::GitTransportConfig {
                git_program: "git".into(),
                workspace: prepared.workspace.clone(),
                home_directory: state.clone(),
                private_directory: state,
                identity: None,
                local_environment: Some(prepared.native_environment.clone()),
                deadline: std::time::Duration::from_secs(30),
            },
            &repo.remote,
        );
        let head = git_output(&prepared.workspace, &["rev-parse", "HEAD"]);
        transport
            .push(
                branch,
                &head,
                crate::source_code_provider::SourceDeliveryCredential::token(""),
            )
            .await
            .assert_value();
        assert_eq!(git_output(&repo.remote, &["rev-parse", branch]), head);
        assert_eq!(git_output(&repo.workspace, &["rev-parse", "HEAD"]), local);
        assert_eq!(
            git_output(&repo.workspace, &["symbolic-ref", "--short", "HEAD"]),
            "feature/local"
        );
        assert_eq!(git_output(&repo.workspace, &["write-tree"]), index);
        assert_eq!(
            git_output(&repo.workspace, &["status", "--porcelain=v1"]),
            status
        );
        assert!(prepared.workspace.is_dir());
    }
}

#[tokio::test]
async fn remote_base_requires_an_upstream_and_review_targets_distinguish_modes() {
    use crate::native_v2_delivery::DeliveryMode;
    let repo = TestGitRepository::candidate();
    git(&repo.workspace, &["checkout", "-b", "feature/new"]);
    let mut run = prepared(&repo, Some(DeliveryMode::PullRequest));
    redirect_remote(&repo);
    let state = repo.root.child("state");
    assert!(matches!(
        prepare_branch_source(
            &mut run,
            Some(LocalWorktreeBase::Remote),
            Path::new("git"),
            &state
        )
        .await,
        Err(LocalCompositionError::MissingWorktreeUpstream)
    ));
    for mode in [DeliveryMode::PullRequest, DeliveryMode::Merge] {
        let mut run = prepared(&repo, Some(mode));
        prepare_branch_source(&mut run, None, Path::new("git"), &state)
            .await
            .assert_value();
        assert_eq!(run.local_branch.as_deref(), Some("feature/new"));
        assert_eq!(
            run.submission.source.as_ref().unwrap().branch.as_str(),
            "main"
        );
        git(&repo.workspace, &["checkout", "main"]);
        let mut run = prepared(&repo, Some(mode));
        let error = prepare_branch_source(&mut run, None, Path::new("git"), &state)
            .await
            .expect_err("target branch must refuse");
        let message = error.to_string();
        assert!(
            message.contains("main")
                && message.contains("feature branch")
                && message.contains("--worktree")
        );
        prepare_branch_source(
            &mut run,
            Some(LocalWorktreeBase::Local),
            Path::new("git"),
            &state,
        )
        .await
        .assert_value();
        git(&repo.workspace, &["checkout", "feature/new"]);
    }
}

#[test]
fn detached_resume_reattaches_the_run_branch_without_rewinding_it() {
    let repo = TestGitRepository::candidate();
    git(&repo.workspace, &["checkout", "-b", "feature/resume"]);
    std::fs::write(repo.workspace.join("README.md"), "later commit\n").assert_value();
    let live = commit(&repo, "later");
    git(&repo.workspace, &["checkout", "--detach", &repo.base]);
    let message = source::attached_branch(&repo.workspace, Path::new("git"))
        .expect_err("detached")
        .to_string();
    assert!(message.contains("HEAD is detached") && message.contains("git switch feature/resume"));
    attach_run_branch(&repo.workspace, Path::new("git"), "feature/resume").assert_value();
    assert_eq!(
        git_output(&repo.workspace, &["symbolic-ref", "--short", "HEAD"]),
        "feature/resume"
    );
    assert_eq!(git_output(&repo.workspace, &["rev-parse", "HEAD"]), live);
    assert_eq!(
        std::fs::read_to_string(repo.workspace.join("README.md")).assert_value(),
        "base\n"
    );
}

#[test]
fn resume_refuses_a_branch_owned_by_another_checkout_without_changing_either() {
    for main_checkout in [false, true] {
        let repo = TestGitRepository::candidate();
        let branch = "zeroshot/v2-retained";
        let retained = repo.root.child("retained worktree");
        git(
            &repo.workspace,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                retained.to_str().assert_value(),
                &repo.base,
            ],
        );
        git(&retained, &["checkout", "--detach"]);
        let owner = if main_checkout {
            git(&repo.workspace, &["checkout", branch]);
            repo.workspace.clone()
        } else {
            #[cfg(unix)]
            let owner = repo.root.child("other checkout\nwith newline");
            #[cfg(not(unix))]
            let owner = repo.root.child("other checkout");
            git(
                &repo.workspace,
                &["worktree", "add", owner.to_str().assert_value(), branch],
            );
            owner
        };
        // The other checkout can advance the shared tip while the run remains detached.
        std::fs::write(owner.join("README.md"), "later shared commit\n").assert_value();
        git(
            &owner,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-am",
                "later commit",
            ],
        );
        let live = git_output(&owner, &["rev-parse", "HEAD"]);
        for checkout in [&retained, &owner] {
            std::fs::write(checkout.join("README.md"), "staged candidate\n").assert_value();
            git(checkout, &["add", "README.md"]);
            std::fs::write(checkout.join("README.md"), "unstaged candidate\n").assert_value();
        }
        let snapshot = |checkout: &Path| {
            let index = git_output(
                checkout,
                &["rev-parse", "--path-format=absolute", "--git-path", "index"],
            );
            let head = git_output(
                checkout,
                &["rev-parse", "--path-format=absolute", "--git-path", "HEAD"],
            );
            (
                std::fs::read(head).assert_value(),
                std::fs::read(index).assert_value(),
                std::fs::read(checkout.join("README.md")).assert_value(),
                git_output(checkout, &["show-ref"]),
            )
        };
        let retained_before = snapshot(&retained);
        let owner_before = snapshot(&owner);
        let error = attach_run_branch(&retained, Path::new("git"), branch)
            .expect_err("another checkout owns the run branch")
            .to_string();
        let expected_owner = std::fs::canonicalize(&owner).assert_value();
        assert!(
            error.contains(branch) && error.contains(expected_owner.to_str().assert_value()),
            "{error}"
        );
        assert_eq!(snapshot(&retained), retained_before);
        assert_eq!(snapshot(&owner), owner_before);

        // Explicitly releasing ownership permits attachment, without rewinding the shared tip.
        git(&owner, &["checkout", "--detach"]);
        attach_run_branch(&retained, Path::new("git"), branch).assert_value();
        assert_eq!(
            git_output(&retained, &["symbolic-ref", "--short", "HEAD"]),
            branch
        );
        assert_eq!(git_output(&retained, &["rev-parse", "HEAD"]), live);
        let after = snapshot(&retained);
        assert_eq!(after.1, retained_before.1);
        assert_eq!(after.2, retained_before.2);
        assert_eq!(after.3, retained_before.3);
    }
}

#[tokio::test]
async fn a_worktree_without_delivery_needs_no_remote() {
    let repo = TestGitRepository::candidate();
    git(&repo.workspace, &["remote", "remove", "origin"]);
    let mut request = local_request(RunId::new("source-less-worktree"));
    request.worktree = Some(LocalWorktreeBase::Local);
    let mut prepared = prepare_local_run(request, &repo.workspace, Path::new("git")).assert_value();
    let state = repo.root.child("state");
    prepare_branch_source(
        &mut prepared,
        Some(LocalWorktreeBase::Local),
        Path::new("git"),
        &state,
    )
    .await
    .assert_value();
    assert!(prepared.submission.source.is_none());
    assert_eq!(
        prepared.workspace_revision.as_ref().unwrap().as_str(),
        repo.base
    );
    create_run_worktree(&mut prepared, Path::new("git"), &state)
        .await
        .assert_value();
    assert_eq!(
        git_output(&prepared.workspace, &["rev-parse", "HEAD"]),
        repo.base
    );
}

#[test]
fn detached_restore_refusal_names_the_last_attached_destination() {
    let repo = TestGitRepository::candidate();
    git(&repo.workspace, &["checkout", "-b", "feature/restored"]);
    // Legacy checkpoint restore wrote the commit directly, without a checkout reflog entry.
    std::fs::write(repo.workspace.join(".git/HEAD"), format!("{}\n", repo.base)).assert_value();
    let message = source::attached_branch(&repo.workspace, Path::new("git"))
        .expect_err("detached restored HEAD")
        .to_string();
    assert!(
        message.contains("HEAD is detached") && message.contains("git switch feature/restored"),
        "{message}"
    );
}
