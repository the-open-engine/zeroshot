//! A plain local resume restarts the graph on the checkout as it is now, so work done after a
//! failure survives even though a latest snapshot exists. Restoring a selected checkpoint is an
//! explicit rollback: without `--discard-workspace-changes` it is refused before anything changes.

use super::*;

const IGNORED: &str = "build.log";

#[tokio::test(flavor = "multi_thread")]
async fn plain_resume_keeps_the_work_done_after_a_failure() {
    let fixture = LocalFixture::new();
    ignore_build_logs(&fixture);
    let failed = fixture.failed_run("Plain resume keeps later work").await;
    assert!(
        fixture
            .run_storage(&failed)
            .join("checkpoints")
            .join("latest.json")
            .is_file(),
        "precondition: the failed run published a latest snapshot"
    );
    let work = LaterWork::record(&fixture);

    let resumed = fixture.json(&["resume", &failed], "finish").await;
    let successor = resumed
        .assert_key("runId")
        .as_str()
        .assert_value_with("successor run identity")
        .to_owned();
    fixture.wait_succeeded(&successor, "finish").await;

    work.assert_kept(&fixture);
    let turns = fs::read_to_string(fixture.repository.join("local-session.args"))
        .assert_value_with("local session arguments");
    assert!(
        turns.lines().any(|line| line == "mode=finish"),
        "the successor did not restart the graph at its worker:\n{turns}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_rollback_requires_consent_and_changes_nothing_without_it() {
    let fixture = LocalFixture::new();
    ignore_build_logs(&fixture);
    let snapshot_head = git(&fixture.repository, &["rev-parse", "HEAD"]);
    let failed = fixture.failed_run("Checkpoint rollback consent").await;
    let checkpoint_id = fixture
        .checkpoints(&failed)
        .await
        .iter()
        .find(|checkpoint| checkpoint.assert_key("node") == "worker")
        .assert_value_with("the failed run captured a worker checkpoint")
        .assert_key("checkpointId")
        .as_str()
        .assert_value_with("checkpoint identity")
        .to_owned();
    let work = LaterWork::record(&fixture);

    let refused = fixture
        .output(
            &["resume", &failed, "--from-checkpoint", &checkpoint_id],
            "finish",
        )
        .await;
    let stderr = String::from_utf8_lossy(&refused.stderr);
    let opt_in = format!(
        "zeroshot resume {failed} --from-checkpoint {checkpoint_id} --discard-workspace-changes"
    );
    assert!(
        !refused.status.success()
            && refused.stdout.is_empty()
            && stderr.contains(&opt_in)
            && ["staged", "untracked", "ignored", "HEAD"]
                .iter()
                .all(|loss| stderr.contains(loss)),
        "a rollback without consent must explain what it replaces and name {opt_in:?}\n\
         stderr:\n{stderr}"
    );
    work.assert_kept(&fixture);
    let recovery = fixture
        .json(&["status", &failed], "finish")
        .await
        .assert_key("workspaceRecovery")
        .clone();
    assert!(
        recovery.assert_key("recoverable") == true && recovery.get("successorRunId").is_none(),
        "a refused rollback must not claim a successor: {recovery}"
    );
    assert_eq!(
        fixture
            .json(&["list"], "finish")
            .await
            .assert_key("runs")
            .as_array()
            .assert_value_with("run list")
            .len(),
        1,
        "a refused rollback created a run"
    );

    let restored = fixture
        .json(
            &[
                "resume",
                &failed,
                "--from-checkpoint",
                &checkpoint_id,
                "--discard-workspace-changes",
            ],
            "finish",
        )
        .await;
    let successor = restored
        .assert_key("runId")
        .as_str()
        .assert_value_with("successor run identity")
        .to_owned();
    fixture.wait_succeeded(&successor, "finish").await;
    let repository = &fixture.repository;
    for later in ["fix.txt", "staged.txt", "untracked.txt", IGNORED] {
        assert!(
            !repository.join(later).exists(),
            "the rollback kept {later}"
        );
    }
    assert_eq!(
        fs::read_to_string(repository.join("seed.txt")).assert_value_with("restored seed"),
        "seed\n"
    );
    // The checkpoint installs its own HEAD and index; shared branch refs keep the later commit.
    assert_eq!(git(repository, &["rev-parse", "HEAD"]), snapshot_head);
    assert_eq!(
        git(repository, &["rev-parse", "refs/heads/feature/local"]),
        work.head
    );
}

fn ignore_build_logs(fixture: &LocalFixture) {
    fs::write(
        fixture.repository.join(".gitignore"),
        format!("{IGNORED}\n"),
    )
    .assert_value_with("write ignore rules");
    git(&fixture.repository, &["add", ".gitignore"]);
    git(&fixture.repository, &["commit", "-m", "ignore build logs"]);
}

/// Work a user does in the retained checkout after the failure.
struct LaterWork {
    head: String,
    index: String,
    status: String,
}

impl LaterWork {
    fn record(fixture: &LocalFixture) -> Self {
        let repository = &fixture.repository;
        fs::write(repository.join("fix.txt"), "committed fix\n").assert_value_with("write fix");
        git(repository, &["add", "fix.txt"]);
        git(repository, &["commit", "-m", "fix after the failure"]);
        fs::write(repository.join("staged.txt"), "staged\n").assert_value_with("write staged");
        git(repository, &["add", "staged.txt"]);
        fs::write(repository.join("seed.txt"), "unstaged edit\n").assert_value_with("edit seed");
        fs::write(repository.join("untracked.txt"), "untracked\n")
            .assert_value_with("write untracked");
        fs::write(repository.join(IGNORED), "ignored\n").assert_value_with("write ignored");
        Self {
            head: git(repository, &["rev-parse", "HEAD"]),
            index: git(repository, &["ls-files", "--stage"]),
            status: workspace_status(repository),
        }
    }

    fn assert_kept(&self, fixture: &LocalFixture) {
        let repository = &fixture.repository;
        for (path, contents) in [
            ("fix.txt", "committed fix\n"),
            ("staged.txt", "staged\n"),
            ("seed.txt", "unstaged edit\n"),
            ("untracked.txt", "untracked\n"),
            (IGNORED, "ignored\n"),
        ] {
            assert_eq!(
                fs::read_to_string(repository.join(path)).ok().as_deref(),
                Some(contents),
                "the later work in {path} was not kept"
            );
        }
        assert_eq!(git(repository, &["rev-parse", "HEAD"]), self.head);
        assert_eq!(
            git(repository, &["symbolic-ref", "HEAD"]),
            "refs/heads/feature/local",
            "the checkout's branch was detached"
        );
        assert_eq!(git(repository, &["ls-files", "--stage"]), self.index);
        assert_eq!(workspace_status(repository), self.status);
    }
}

fn workspace_status(repository: &Path) -> String {
    git(
        repository,
        &[
            "status",
            "--porcelain",
            "--ignored",
            "--untracked-files=all",
        ],
    )
}
