#![cfg(unix)]

//! A local run in a repository that Git still accepts must not stop at its first workspace
//! checkpoint with a bare `No such file or directory`.
//!
//! Leftover linked worktrees break in ways Git tolerates. Moving the main checkout leaves a nested
//! worktree whose `.git` file names a missing administrative directory. Moving a linked worktree
//! leaves its administrative backpointer naming the old location; that worktree may be nested in
//! the repository or be the selected workspace itself. In each state the foreground `zeroshot run`
//! from the first-run guide must either start, or fail with output that names the blocking path and
//! the Git command that repairs or removes the stale link. Each test first proves that Git still
//! accepts the workspace, so the refusal is Zeroshot's.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use openengine_cluster_testkit::assertions::{AssertValue, JsonAt};
use serde_json::Value;
use tokio::time::{Instant, sleep};

#[allow(
    dead_code,
    reason = "this target uses only the shared fixture's foreground run surface"
)]
#[path = "native_v2_cli_local/fixture.rs"]
mod fixture;
use fixture::*;

const TITLE: &str = "Stale worktree checkpoint";
/// Git's own remedies for broken worktree links: `repair`, `remove`, and `prune`.
const GIT_REMEDY: &str = "git worktree";
const STATUS_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::test(flavor = "multi_thread")]
async fn nested_worktree_of_a_moved_checkout_starts_or_names_its_path() {
    let fixture = LocalFixture::new();
    let before = fixture_root(&fixture).join("repository-before-move");
    fs::rename(&fixture.repository, &before).assert_value_with("move the checkout aside");
    git(
        &before,
        &[
            "worktree",
            "add",
            "-b",
            "review",
            ".claude/worktrees/review",
        ],
    );
    fs::rename(&before, &fixture.repository).assert_value_with("move the checkout back");
    let nested = fixture.repository.join(".claude/worktrees/review");
    let marker = fs::read_to_string(nested.join(".git")).assert_value_with("nested .git file");
    let admin = marker
        .trim()
        .strip_prefix("gitdir: ")
        .assert_value_with("nested .git file names its Git directory");
    assert!(
        !Path::new(admin).exists(),
        "precondition: the nested .git file must name a missing Git directory: {marker}"
    );
    assert_git_accepts(&fixture.repository);

    let run = fixture.run(TITLE, "finish", false).await;

    assert_run_starts_or_names_blocker(&fixture, &run, &[".claude/worktrees/review".to_owned()])
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nested_worktree_moved_with_mv_starts_or_names_its_path() {
    let fixture = LocalFixture::new();
    git(
        &fixture.repository,
        &[
            "worktree",
            "add",
            "-b",
            "review",
            ".claude/worktrees/review",
        ],
    );
    let worktrees = fixture.repository.join(".claude/worktrees");
    fs::rename(worktrees.join("review"), worktrees.join("renamed"))
        .assert_value_with("move the linked worktree");
    assert_stale_backpointer(&fixture.repository.join(".git/worktrees/review"));
    assert_git_accepts(&fixture.repository);
    assert_git_accepts(&worktrees.join("renamed"));

    let run = fixture.run(TITLE, "finish", false).await;

    assert_run_starts_or_names_blocker(&fixture, &run, &[".claude/worktrees/renamed".to_owned()])
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_that_is_a_moved_linked_worktree_starts_or_names_its_path() {
    let fixture = LocalFixture::new();
    let root = fixture_root(&fixture);
    let main = root.join("repository-main");
    let before = root.join("repository-before-move");
    fs::rename(&fixture.repository, &main).assert_value_with("keep the main checkout");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-b",
            "linked",
            before.to_str().assert_value_with("linked worktree path"),
        ],
    );
    fs::rename(&before, &fixture.repository).assert_value_with("move the linked worktree");
    assert_stale_backpointer(&main.join(".git/worktrees/repository-before-move"));
    assert_git_accepts(&fixture.repository);

    let run = fixture.run(TITLE, "finish", false).await;

    // The blocking path is the selected workspace or the old location its Git link still records.
    let workspace = fs::canonicalize(&fixture.repository).assert_value_with("canonical workspace");
    let names = [
        fixture.repository.display().to_string(),
        workspace.display().to_string(),
        "repository-before-move".to_owned(),
    ];
    assert_run_starts_or_names_blocker(&fixture, &run, &names).await;
}

fn fixture_root(fixture: &LocalFixture) -> PathBuf {
    fixture
        .repository
        .parent()
        .assert_value_with("fixture root")
        .to_owned()
}

/// Git still resolves the checkout and reports its status, so the repository is usable.
fn assert_git_accepts(checkout: &Path) {
    let top = git(checkout, &["rev-parse", "--show-toplevel"]);
    assert_eq!(
        fs::canonicalize(top).assert_value_with("Git top level"),
        fs::canonicalize(checkout).assert_value_with("checkout"),
        "precondition: Git must resolve the checkout itself"
    );
    git(checkout, &["status", "--porcelain"]);
}

fn assert_stale_backpointer(admin: &Path) {
    let recorded = fs::read_to_string(admin.join("gitdir")).assert_value_with("worktree gitdir");
    assert!(
        !Path::new(recorded.trim()).exists(),
        "precondition: the worktree backpointer must name its old, missing location: {recorded}"
    );
}

async fn assert_run_starts_or_names_blocker(
    fixture: &LocalFixture,
    run: &Output,
    names: &[String],
) {
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    let run_id = stdout
        .lines()
        .next()
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .and_then(|receipt| receipt.get("runId")?.as_str().map(str::to_owned));
    let mut status = None;
    if let Some(run_id) = run_id {
        let finished = finished_status(fixture, &run_id).await;
        if finished
            .assert_key("status")
            .assert_key("terminalResult")
            .assert_key("status")
            == "succeeded"
        {
            return;
        }
        status = Some(finished);
    }
    let output = format!("{stdout}\n{stderr}");
    let named = names.iter().any(|name| output.contains(name.as_str()));
    assert!(
        named && output.contains(GIT_REMEDY),
        "a repository Git accepts neither started nor named the blocking path and its remedy: \
         the `zeroshot run` output must name one of {names:?} and a `{GIT_REMEDY}` command\n\
         --- zeroshot run exit ---\n{}\n--- zeroshot run stdout ---\n{stdout}\n\
         --- zeroshot run stderr ---\n{stderr}\n--- final status ---\n{status:?}",
        run.status
    );
}

async fn finished_status(fixture: &LocalFixture, run_id: &str) -> Value {
    let deadline = Instant::now() + STATUS_TIMEOUT;
    loop {
        let status = fixture.json(&["status", run_id], "finish").await;
        if status.assert_key("status").assert_key("phase") == "finished" {
            return status;
        }
        assert!(Instant::now() < deadline, "run did not finish: {status}");
        sleep(Duration::from_millis(30)).await;
    }
}
