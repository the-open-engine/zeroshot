use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

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
        assert_conflict_resolution(resolution, false).await;
    }
}

#[tokio::test]
async fn concluded_conflict_merge_carries_the_runs_resolution_and_one_credit() {
    for resolution in ["combined change\n", "local change\n"] {
        assert_conflict_resolution(resolution, true).await;
    }
}

async fn assert_conflict_resolution(resolution: &str, credited: bool) {
    use crate::native_v2_candidate::test_support::{
        CO_AUTHOR, co_author_identity, commit_attribution, git,
    };
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
    let system = SystemGit::new(PathBuf::from("git"))
        .with_commit_co_author(credited.then(co_author_identity));
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
    let credit = if credited {
        format!("\n\nCo-authored-by: {CO_AUTHOR}")
    } else {
        String::new()
    };
    let author = "Zeroshot <delivery@zeroshot.invalid>";
    assert_eq!(
        commit_attribution(workspace, "HEAD"),
        format!("{author}|{author}\nresolved{credit}")
    );
}

#[tokio::test]
async fn delivery_commit_identity_overrides_repository_author_and_committer() {
    let repository = TestGitRepository::delivery();
    for (key, value) in [
        ("author.name", "Caller Author"),
        ("author.email", "author@example.invalid"),
        ("committer.name", "Caller Committer"),
        ("committer.email", "committer@example.invalid"),
    ] {
        crate::native_v2_candidate::test_support::git(
            &repository.workspace,
            &["config", "--local", key, value],
        );
    }
    let revision = SystemGit::new("git".into())
        .prepare_revision(&repository.workspace, &repository.base, "delivery identity")
        .await
        .assert_value();
    assert_eq!(
        git_output(
            &repository.workspace,
            &["show", "-s", "--format=%an <%ae>|%cn <%ce>", &revision]
        ),
        "Zeroshot <delivery@zeroshot.invalid>|Zeroshot <delivery@zeroshot.invalid>"
    );
}

#[tokio::test]
async fn host_commit_identity_authors_and_commits_the_prepared_revision() {
    use crate::native_v2_candidate::test_support::{BOT_AUTHOR, bot_commit_identity, commit_authorship};
    let repository = TestGitRepository::delivery();
    crate::native_v2_candidate::test_support::git(
        &repository.workspace,
        &["config", "--local", "user.name", "Caller"],
    );
    let revision = SystemGit::new("git".into())
        .with_commit_identity(Some(bot_commit_identity()))
        .prepare_revision(&repository.workspace, &repository.base, "bot identity")
        .await
        .assert_value();
    assert_eq!(
        commit_authorship(&repository.workspace, &revision),
        format!("{BOT_AUTHOR}|{BOT_AUTHOR}")
    );
}

#[tokio::test]
async fn co_author_trailer_credits_each_prepared_revision_once_and_keeps_its_authorship() {
    use crate::native_v2_candidate::test_support::{
        BOT_AUTHOR, CO_AUTHOR, bot_commit_identity, co_author_identity, commit_attribution,
    };
    let credit = format!("Co-authored-by: {CO_AUTHOR}");
    let trailers = "Signed-off-by: Starter <starter@example.com>\n\
                    Reviewed-by: Reviewer <reviewer@example.com>";
    let messages = [
        (
            "fix: one line".to_owned(),
            format!("fix: one line\n\n{credit}"),
        ),
        (
            format!("feat: trailers\n\nBody.\n\n{trailers}"),
            format!("feat: trailers\n\nBody.\n\n{trailers}\n{credit}"),
        ),
        (
            format!("fix: credited\n\n{credit}\n{trailers}"),
            format!("fix: credited\n\n{credit}\n{trailers}"),
        ),
    ];
    for (identity, author) in [
        (None, "Zeroshot <delivery@zeroshot.invalid>"),
        (Some(bot_commit_identity()), BOT_AUTHOR),
    ] {
        let repository = TestGitRepository::delivery();
        // Workspace trailer configuration must not rename, move, repeat or drop the credit.
        for (key, value) in [
            ("trailer.ifexists", "add"),
            ("trailer.co-authored-by.ifexists", "add"),
            ("trailer.co-authored-by.key", "Helped-by"),
            ("trailer.cab.key", "Co-authored-by"),
            ("trailer.cab.ifexists", "add"),
            ("trailer.where", "start"),
            ("trailer.ifmissing", "doNothing"),
            ("trailer.separators", "#"),
        ] {
            crate::native_v2_candidate::test_support::git(
                &repository.workspace,
                &["config", "--local", key, value],
            );
        }
        let git = SystemGit::new("git".into())
            .with_commit_identity(identity)
            .with_commit_co_author(Some(co_author_identity()));
        for (index, (message, expected)) in messages.iter().enumerate() {
            std::fs::write(
                repository.workspace.join(format!("change-{index}")),
                "change\n",
            )
            .assert_value();
            let revision = git
                .prepare_revision(&repository.workspace, &repository.base, message)
                .await
                .assert_value();
            assert_eq!(
                commit_attribution(&repository.workspace, &revision),
                format!("{author}|{author}\n{expected}")
            );
        }
    }
}

#[tokio::test]
async fn co_author_credit_never_lets_git_accept_an_empty_message() {
    use crate::native_v2_candidate::test_support::{
        CO_AUTHOR, co_author_identity, commit_attribution, git,
    };
    let author = "Zeroshot <delivery@zeroshot.invalid>";
    // Git's whitespace cleanup empties these, so Git refuses them with or without a credit.
    let empty = [
        (None, ""),
        (None, "   "),
        (None, "\n\t\r\n"),
        (None, "Signed-off-by: Starter <starter@example.com>"),
        (
            None,
            "Signed-off-by: A <a@example.com>\n\nSigned-off-by: B <b@example.com> \t",
        ),
    ];
    for (cleanup, message) in empty {
        for co_author in [None, Some(co_author_identity())] {
            assert_refused_as_empty(cleanup, message, co_author).await;
        }
    }
    // A credited commit uses that cleanup whatever the workspace configures.
    assert_refused_as_empty(Some("verbatim"), "   ", Some(co_author_identity())).await;
    for (cleanup, message, kept) in [
        (None, "Signed-off-by: ", "Signed-off-by:"),
        (None, "\nSigned-off-by: \n", "Signed-off-by:"),
        (
            None,
            " Signed-off-by: Starter <starter@example.com>",
            " Signed-off-by: Starter <starter@example.com>",
        ),
        (None, "\u{b}", "\u{b}"),
        (Some("strip"), "#123 fix login", "#123 fix login"),
    ] {
        let repository = TestGitRepository::delivery();
        if let Some(cleanup) = cleanup {
            git(
                &repository.workspace,
                &["config", "--local", "commit.cleanup", cleanup],
            );
        }
        let revision = SystemGit::new("git".into())
            .with_commit_co_author(Some(co_author_identity()))
            .prepare_revision(&repository.workspace, &repository.base, message)
            .await
            .assert_value();
        assert_eq!(
            commit_attribution(&repository.workspace, &revision),
            format!("{author}|{author}\n{kept}\n\nCo-authored-by: {CO_AUTHOR}"),
            "{message:?}"
        );
    }
}

async fn assert_refused_as_empty(
    cleanup: Option<&str>,
    message: &str,
    co_author: Option<openengine_cluster_protocol::CommitIdentity>,
) {
    let credited = co_author.is_some();
    let repository = TestGitRepository::delivery();
    if let Some(cleanup) = cleanup {
        crate::native_v2_candidate::test_support::git(
            &repository.workspace,
            &["config", "--local", "commit.cleanup", cleanup],
        );
    }
    let error = SystemGit::new("git".into())
        .with_commit_co_author(co_author)
        .prepare_revision(&repository.workspace, &repository.base, message)
        .await
        .assert_error();
    assert!(
        matches!(&error, GitError::Command(failure) if failure.stderr.contains("empty commit message")),
        "{message:?} credited={credited}: {error}"
    );
    assert_eq!(
        git_output(&repository.workspace, &["rev-parse", "HEAD"]),
        repository.base
    );
}

#[tokio::test]
async fn failed_credited_commit_leaves_the_change_for_a_credited_retry() {
    use crate::native_v2_candidate::test_support::{CO_AUTHOR, co_author_identity, commit_attribution};
    let repository = TestGitRepository::delivery();
    let configuration = repository.workspace.join(".git/config");
    let original = std::fs::read_to_string(&configuration).assert_value();
    // Git 2.43's trailer handling crashes on a key without a value, failing the commit.
    std::fs::write(
        &configuration,
        format!("{original}[trailer \"ticket\"]\n\tkey\n"),
    )
    .assert_value();
    let git = SystemGit::new("git".into()).with_commit_co_author(Some(co_author_identity()));
    let author = "Zeroshot <delivery@zeroshot.invalid>";
    let credited = format!("{author}|{author}\nfix: login\n\nCo-authored-by: {CO_AUTHOR}");
    match git
        .prepare_revision(&repository.workspace, &repository.base, "fix: login")
        .await
    {
        // A Git that tolerates the setting credits the commit at once.
        Ok(revision) => assert_eq!(
            commit_attribution(&repository.workspace, &revision),
            credited
        ),
        Err(_) => assert_eq!(
            git_output(&repository.workspace, &["rev-parse", "HEAD"]),
            repository.base,
            "a failed credited commit must not leave an uncredited one behind"
        ),
    }
    std::fs::write(&configuration, original).assert_value();
    let revision = git
        .prepare_revision(&repository.workspace, &repository.base, "fix: login")
        .await
        .assert_value();
    assert_eq!(
        commit_attribution(&repository.workspace, &revision),
        credited
    );
}
