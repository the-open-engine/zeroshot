//! A local `resume` checks every lane's executable on its captured PATH, as a local `run` does,
//! before it admits a successor. A resume that restores a workspace checkpoint checks against the
//! restored workspace, so an executable that only the checkpoint contains still resumes.

use openengine_cluster_testkit::assertions::AssertAt;

use super::*;

/// What a local run reports when its captured PATH lacks the fixture lane's executable.
const MISSING_CODEX: &str = "lane codex/openai needs the `codex` executable on PATH";
/// A PATH like the one cron or a systemd unit provides: host tools, but not the fake Codex.
const HOST_PATH: &str = "/usr/bin:/bin";

#[tokio::test(flavor = "multi_thread")]
async fn resume_without_the_lane_executable_on_path_is_refused_like_run() {
    let mut fixture = LocalFixture::new();
    let failed_run_id = failed_run(&fixture).await;
    let checkpoint_id = worker_checkpoint(&fixture, &failed_run_id).await;
    for directory in HOST_PATH.split(':') {
        assert!(
            !Path::new(directory).join("codex").exists(),
            "precondition: {directory} must not contain codex"
        );
    }
    fixture
        .harness_environment
        .insert("PATH".to_owned(), HOST_PATH.to_owned());

    let run = fixture.run("Run without Codex", "finish", false).await;
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        !run.status.success() && stderr.contains(MISSING_CODEX),
        "precondition: run from a PATH without codex must refuse\nstderr:\n{stderr}"
    );

    // A plain resume restores the latest checkpoint; `--from-checkpoint` selects one.
    for selection in [&[][..], &["--from-checkpoint", checkpoint_id.as_str()][..]] {
        let mut arguments = vec!["resume", failed_run_id.as_str()];
        arguments.extend_from_slice(selection);
        let resume = fixture.output(&arguments, "finish").await;
        let (stdout, stderr) = (
            String::from_utf8_lossy(&resume.stdout),
            String::from_utf8_lossy(&resume.stderr),
        );
        assert!(
            !resume.status.success() && stderr.contains(MISSING_CODEX) && stdout.trim().is_empty(),
            "`zeroshot {}` from a PATH without codex must refuse as run does, naming \
             {MISSING_CODEX:?}, and start no successor\nstdout:\n{stdout}\nstderr:\n{stderr}",
            arguments.join(" ")
        );
        let recovery = fixture
            .json(&["status", &failed_run_id], "finish")
            .await
            .assert_key("workspaceRecovery")
            .clone();
        assert!(
            recovery.assert_key("recoverable") == true && recovery.get("successorRunId").is_none(),
            "a refused resume must leave the run resumable without a successor: {recovery}"
        );
    }
    let listed = fixture.json(&["list"], "finish").await;
    let runs = listed
        .assert_key("runs")
        .as_array()
        .assert_value_with("run list")
        .iter()
        .map(|run| run.assert_key("runId").clone())
        .collect::<Vec<_>>();
    assert_eq!(
        runs,
        [json!(failed_run_id)],
        "a refused resume created a run"
    );

    // From a PATH that finds codex, the same run still resumes.
    fixture.harness_environment.remove("PATH");
    let resumed = fixture.output(&["resume", &failed_run_id], "finish").await;
    assert_success(&resumed, "resume from a PATH that finds codex");
    fixture
        .wait_succeeded(&receipt_run_id(&resumed), "finish")
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_finds_a_lane_executable_that_only_its_restored_checkpoint_contains() {
    let mut fixture = LocalFixture::new();
    // The workspace carries its own codex, which a relative PATH entry finds from the workspace.
    let tools = fixture.repository.join("tools");
    fs::create_dir_all(&tools).assert_value_with("create workspace tool directory");
    write_fake_codex(&tools.join("codex"));
    git(&fixture.repository, &["add", "tools/codex"]);
    git(&fixture.repository, &["commit", "-m", "carry codex"]);
    let failed_run_id = failed_run(&fixture).await;
    fs::remove_file(tools.join("codex")).assert_value_with("remove the workspace codex");
    fixture
        .harness_environment
        .insert("PATH".to_owned(), format!("tools:{HOST_PATH}"));

    let resumed = fixture.output(&["resume", &failed_run_id], "finish").await;

    assert_success(
        &resumed,
        "resume whose lane executable only the restored checkpoint contains",
    );
    fixture
        .wait_succeeded(&receipt_run_id(&resumed), "finish")
        .await;
    assert!(
        tools.join("codex").is_file(),
        "the resume did not restore the checkpointed workspace"
    );
}

/// Fails one foreground run, which leaves a recoverable workspace with checkpoints.
async fn failed_run(fixture: &LocalFixture) -> String {
    let failed = fixture.run("Resume lane executable", "fail", false).await;
    assert!(
        !failed.status.success(),
        "injected local failure succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&failed.stdout),
        String::from_utf8_lossy(&failed.stderr)
    );
    let run_id = json_lines(&failed.stdout)
        .as_slice()
        .assert_at(0)
        .assert_key("runId")
        .as_str()
        .assert_value_with("failed run receipt")
        .to_owned();
    wait_for_exit(fixture.ready_pid(&run_id)).await;
    run_id
}

async fn worker_checkpoint(fixture: &LocalFixture, run_id: &str) -> String {
    fixture
        .json(&["checkpoints", run_id], "finish")
        .await
        .assert_key("checkpoints")
        .as_array()
        .assert_value_with("checkpoint list")
        .iter()
        .find(|checkpoint| checkpoint.assert_key("node") == "worker")
        .assert_value_with("the failed run captured a worker checkpoint")
        .assert_key("checkpointId")
        .as_str()
        .assert_value_with("checkpoint identity")
        .to_owned()
}
