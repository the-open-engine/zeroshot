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

    // A plain resume keeps the checkout; `--from-checkpoint` restores the selected one.
    let rollback = [
        "--from-checkpoint",
        checkpoint_id.as_str(),
        "--discard-workspace-changes",
    ];
    for selection in [&[][..], &rollback[..]] {
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

    // From a PATH that finds codex, the same run still resumes. The refused rollback may have
    // left its restore incomplete, so a plain resume is refused until that rollback is repeated.
    fixture.harness_environment.remove("PATH");
    let plain = fixture.output(&["resume", &failed_run_id], "finish").await;
    let stderr = String::from_utf8_lossy(&plain.stderr);
    assert!(
        !plain.status.success() && stderr.contains("partially restored"),
        "a plain resume must not start on a pending restore\nstderr:\n{stderr}"
    );
    let mut arguments = vec!["resume", failed_run_id.as_str()];
    arguments.extend_from_slice(&rollback);
    let resumed = fixture.output(&arguments, "finish").await;
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
    let checkpoint_id = worker_checkpoint(&fixture, &failed_run_id).await;
    fs::remove_file(tools.join("codex")).assert_value_with("remove the workspace codex");
    fixture
        .harness_environment
        .insert("PATH".to_owned(), format!("tools:{HOST_PATH}"));

    let resumed = fixture
        .output(
            &[
                "resume",
                &failed_run_id,
                "--from-checkpoint",
                &checkpoint_id,
                "--discard-workspace-changes",
            ],
            "finish",
        )
        .await;

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

#[tokio::test(flavor = "multi_thread")]
async fn local_run_and_checkpoint_resume_refuse_old_or_unreadable_cli_before_dispatch() {
    let fixture = LocalFixture::new();
    let failed_run_id = failed_run(&fixture).await;
    let checkpoint_id = worker_checkpoint(&fixture, &failed_run_id).await;
    let executable = fixture.root.path("bin/codex");
    let unexpected_turns = fixture.root.path("unexpected-turns");
    for version in ["codex-cli 0.154.0", "unknown"] {
        openengine_cluster_testkit::fixture::write_executable(
            &executable,
            format!("#!/bin/sh\nif [ \"$*\" = --version ]; then printf '%s\\n' '{version}'; exit 0; fi\nprintf turn >> '{}'\nexit 91\n", unexpected_turns.display()),
            0o700,
        )
        .assert_value();
        for arguments in [
            vec![
                "run",
                "--title",
                "Unsupported CLI",
                "--graph",
                fixture.graph.to_str().assert_value(),
                "--input",
                fixture.input.to_str().assert_value(),
                "--runtime-config",
                fixture.runtime.to_str().assert_value(),
            ],
            vec![
                "resume",
                &failed_run_id,
                "--from-checkpoint",
                &checkpoint_id,
                "--discard-workspace-changes",
            ],
        ] {
            let output = fixture.output(&arguments, "finish").await;
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{stderr}");
            for expected in [
                "lane codex/openai",
                "codex",
                version,
                "0.155.0 or newer",
                "npm install -g @openai/codex@latest",
            ] {
                assert!(stderr.contains(expected), "missing {expected}: {stderr}");
            }
            assert!(output.stdout.is_empty());
            assert!(
                !unexpected_turns.exists(),
                "version refusal dispatched a model turn"
            );
        }
        let listed = fixture.json(&["list"], "finish").await;
        assert_eq!(listed.assert_key("runs").as_array().assert_value().len(), 1);
        let recovery = fixture.json(&["status", &failed_run_id], "finish").await;
        assert!(
            recovery
                .assert_key("workspaceRecovery")
                .get("successorRunId")
                .is_none()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn controller_shutdown_cancels_version_probes_before_run_or_checkpoint_resume_admission() {
    let fixture = LocalFixture::new();
    let failed_run_id = failed_run(&fixture).await;
    let checkpoint_id = worker_checkpoint(&fixture, &failed_run_id).await;
    let pids = fixture.root.path("version-probe-pids");
    let turns = fixture.root.path("unexpected-turns");
    // Linux's process runner inserts a private subreaper between the controller and the CLI.
    #[cfg(target_os = "linux")]
    let controller = "$(/bin/ps -o ppid= -p \"$PPID\")";
    #[cfg(not(target_os = "linux"))]
    let controller = "$PPID";
    openengine_cluster_testkit::fixture::write_executable(
        &fixture.root.path("bin/codex"),
        format!(
            "#!/bin/sh\nif [ \"$*\" != --version ]; then printf turn >> {turns}; exit 91; fi\n\
             printf 'codex-cli 0.155.0\\n'\n/bin/sleep 60 &\n\
             printf '%s %s %s %s\\n' \"{controller}\" \"$PPID\" \"$$\" \"$!\" > {pids}\nwait\n",
            turns = shell_literal(turns.to_str().assert_value()),
            pids = shell_literal(pids.to_str().assert_value()),
        ),
        0o700,
    )
    .assert_value();
    for arguments in [
        vec![
            "run",
            "--title",
            "Cancelled version probe",
            "--graph",
            fixture.graph.to_str().assert_value(),
            "--input",
            fixture.input.to_str().assert_value(),
            "--runtime-config",
            fixture.runtime.to_str().assert_value(),
        ],
        vec![
            "resume",
            &failed_run_id,
            "--from-checkpoint",
            &checkpoint_id,
            "--discard-workspace-changes",
        ],
    ] {
        let child = fixture
            .command(&arguments, "finish")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .assert_value();
        let processes = timeout(CLI_TIMEOUT, async {
            loop {
                if let Ok(text) = fs::read_to_string(&pids) {
                    let processes = text
                        .split_whitespace()
                        .map(|pid| pid.parse::<u32>().assert_value())
                        .collect::<Vec<_>>();
                    if processes.len() == 4 {
                        break processes;
                    }
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .assert_value_with("the version probe must start");
        signal(processes[0], libc::SIGTERM).assert_value();
        let output = timeout(CLI_TIMEOUT, child.wait_with_output())
            .await
            .assert_value_with("shutdown must settle before the controller's 30-second deadline")
            .assert_value();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{stderr}");
        assert!(
            stderr.contains("CLI version probe was cancelled"),
            "{stderr}"
        );
        assert!(
            !stderr.contains("cleanup could not be confirmed"),
            "{stderr}"
        );
        assert!(output.stdout.is_empty(), "cancelled startup admitted a run");
        assert!(!turns.exists(), "cancelled startup dispatched a model turn");
        for pid in &processes[2..] {
            assert!(
                !process_exists(*pid),
                "startup reported before probe process {pid} cleanup"
            );
        }
        for pid in processes {
            wait_for_exit(pid).await;
        }
        fs::remove_file(&pids).assert_value();
        let runs = fixture.json(&["list"], "finish").await;
        assert_eq!(runs.assert_key("runs").as_array().assert_value().len(), 1);
        let status = fixture.json(&["status", &failed_run_id], "finish").await;
        let recovery = status.assert_key("workspaceRecovery");
        assert_eq!(recovery.assert_key("recoverable"), true);
        assert!(recovery.get("successorRunId").is_none());
    }
}
