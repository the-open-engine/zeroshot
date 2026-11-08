//! A checkpoint restore that outlasts the former ten-second controller start-up wait still resumes
//! in both local forms: plain resume from the latest snapshot and `--from-checkpoint`. Restores
//! that a deadline or a crash interrupts keep recovery intact for a retry.

use openengine_cluster_testkit::assertions::AssertAt;

use super::restic_selector::test_restic;
use super::*;

/// Seconds the real Restic restore is delayed; longer than the former ten-second readiness wait.
const RESTORE_DELAY_SECONDS: u32 = 12;
/// Bounds one resume command well beyond the delay, so a failure reports the CLI's own outcome.
const SLOW_RESUME_TIMEOUT: Duration = Duration::from_secs(120);
const AFTER_FAILURE: &str = "after-failure.txt";
/// A configured preparation deadline that ends while the delayed restore is still extracting.
const SHORT_PREPARATION_SECONDS: u64 = 5;
/// The reconciliation wait for a successor whose launcher may still be starting its controller.
const CLAIM_HANDOFF: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread")]
async fn plain_resume_survives_a_restore_slower_than_the_former_startup_wait() {
    let slow = SlowRestore::failed_run().await;
    let successor = slow.resume(&[]).await;
    slow.assert_restored_and_succeeded(&successor).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_resume_survives_a_restore_slower_than_the_former_startup_wait() {
    let slow = SlowRestore::failed_run().await;
    let checkpoint_id = slow.worker_checkpoint().await;
    let successor = slow.resume(&["--from-checkpoint", &checkpoint_id]).await;
    slow.assert_restored_and_succeeded(&successor).await;
}

/// Extraction stops twice: at a configured deadline, which stops the delayed Restic tree, and in
/// a crash of the CLI, controller and Restic, which leaves only that attempt's stage. Neither
/// touches the checkout, and the next resume removes that stage and restores the same checkpoint.
#[tokio::test(flavor = "multi_thread")]
async fn interrupted_checkpoint_restores_keep_recovery_for_a_retry_of_the_same_checkpoint() {
    // While `slow` exists, a restore records its processes and waits until it is stopped. An
    // unstopped wait ends without restoring, so a leaked wrapper cannot write a stage later.
    let slow = SlowRestore::failed_run_through(|restores, restic| {
        let directory = restores.parent().assert_value_with("delayed Restic directory");
        format!(
            "#!/bin/sh\nfor argument in \"$@\"; do\n  if test \"$argument\" = restore; then\n    \
             printf 'restore\\n' >> {log}\n    if test -f {slow}; then\n      /bin/sleep 60 &\n      \
             printf '%s %s %s\\n' \"$$\" \"$PPID\" \"$!\" >> {pids}\n      wait \"$!\"\n      \
             exit 1\n    fi\n    break\n  fi\ndone\nexec {restic} \"$@\"\n",
            log = shell_literal(restores.to_str().assert_value_with("restore log path")),
            slow = shell_literal(&directory.join("slow").display().to_string()),
            pids = shell_literal(&directory.join("pids").display().to_string()),
            restic = shell_literal(restic.to_str().assert_value_with("restic path")),
        )
    })
    .await;
    let checkpoint_id = slow.worker_checkpoint().await;
    let resume = [
        "resume",
        slow.failed_run_id.as_str(),
        "--from-checkpoint",
        &checkpoint_id,
    ];
    fs::write(slow.control("slow"), "").assert_value_with("delay later restores");

    let started = Instant::now();
    let expired = timeout(
        SLOW_RESUME_TIMEOUT,
        slow.fixture
            .command(&resume, "finish")
            .env(
                "ZEROSHOT_PREPARATION_TIMEOUT",
                SHORT_PREPARATION_SECONDS.to_string(),
            )
            .output(),
    )
    .await
    .assert_value_with("expired resume deadline")
    .assert_value_with("start expired resume");
    let elapsed = started.elapsed();
    let stderr = String::from_utf8_lossy(&expired.stderr);
    assert!(
        !expired.status.success()
            && stderr.contains("extracting the snapshot reached the preparation deadline")
            && stderr.contains("ZEROSHOT_PREPARATION_TIMEOUT"),
        "an expired restore must name its step and deadline\nstderr:\n{stderr}"
    );
    // The stopped controller is known to be gone, so its claim is released without the handoff
    // wait that an unknown launcher still receives.
    assert!(
        elapsed < Duration::from_secs(SHORT_PREPARATION_SECONDS) + CLAIM_HANDOFF,
        "an expired restore returned after {elapsed:?}"
    );
    for pid in slow.restore_processes(1).await {
        wait_for_exit(pid).await;
    }
    slow.assert_checkout_untouched();
    let status = slow
        .fixture
        .json(&["status", &slow.failed_run_id], "finish")
        .await;
    assert_eq!(
        status
            .assert_key("workspaceRecovery")
            .assert_key("recoverable"),
        true
    );

    let mut crashed = slow
        .fixture
        .command(&resume, "finish")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .assert_value_with("start the resume that crashes");
    let [wrapper, controller, sleeper] = slow.restore_processes(2).await;
    crashed
        .start_kill()
        .assert_value_with("kill the resume CLI");
    crashed
        .wait()
        .await
        .assert_value_with("reap the resume CLI");
    signal(controller, libc::SIGKILL).assert_value_with("kill the restoring controller");
    // The delayed Restic leads its own process group.
    assert_eq!(unsafe { libc::kill(-(wrapper as i32), libc::SIGKILL) }, 0);
    for pid in [controller, wrapper, sleeper] {
        wait_for_exit(pid).await;
    }
    assert!(slow.fixture.repository.join(AFTER_FAILURE).exists());
    let stages = workspace_siblings(&slow.fixture.repository)
        .into_iter()
        .filter(|name| !slow.siblings.contains(name))
        .collect::<Vec<_>>();
    assert!(
        matches!(stages.as_slice(), [stage] if stage.to_string_lossy().starts_with(".zeroshot-restore-")),
        "a crashed restore must leave exactly its own stage, found {stages:?}"
    );

    fs::remove_file(slow.control("slow")).assert_value_with("restore at full speed");
    let successor = slow.resume(&resume[2..]).await;
    slow.assert_restored_and_succeeded(&successor).await;
    assert_eq!(
        git(&slow.fixture.repository, &["rev-parse", "HEAD"]),
        slow.fixture.head
    );
}

struct SlowRestore {
    fixture: LocalFixture,
    restores: PathBuf,
    failed_run_id: String,
    siblings: Vec<std::ffi::OsString>,
}

impl SlowRestore {
    /// Fails one run whose checkpoints are stored through a real Restic that delays `restore`.
    async fn failed_run() -> Self {
        Self::failed_run_through(|restores, restic| {
            format!(
                "#!/bin/sh\nfor argument in \"$@\"; do\n  if test \"$argument\" = restore; then\n    \
                 printf 'restore\\n' >> {log}\n    /bin/sleep {RESTORE_DELAY_SECONDS}\n    break\n  \
                 fi\ndone\nexec {restic} \"$@\"\n",
                log = shell_literal(restores.to_str().assert_value_with("restore log path")),
                restic = shell_literal(restic.to_str().assert_value_with("restic path")),
            )
        })
        .await
    }

    /// Fails one run whose checkpoints are stored through a wrapper around the real Restic.
    /// `script` receives the restore log and the real Restic and returns the wrapper.
    async fn failed_run_through(script: impl FnOnce(&Path, &Path) -> String) -> Self {
        let mut fixture = LocalFixture::new();
        let directory = fixture.root.path("slow-restic");
        fs::create_dir_all(&directory).assert_value_with("create delayed Restic directory");
        let restores = directory.join("restores.log");
        let wrapper = directory.join("restic");
        openengine_cluster_testkit::fixture::write_executable(
            &wrapper,
            script(&restores, &test_restic()),
            0o755,
        )
        .assert_value_with("write delayed Restic");
        // Checkpoint storage records this program, so every later restore of the lineage uses it.
        fixture
            .harness_environment
            .insert("ZEROSHOT_RESTIC".to_owned(), wrapper.display().to_string());

        let failed = fixture.run("Slow checkpoint restore", "fail", false).await;
        assert!(
            !failed.status.success(),
            "injected local failure succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&failed.stdout),
            String::from_utf8_lossy(&failed.stderr)
        );
        let failed_run_id = json_lines(&failed.stdout)
            .as_slice()
            .assert_at(0)
            .assert_key("runId")
            .as_str()
            .assert_value_with("failed run receipt")
            .to_owned();
        wait_for_exit(fixture.ready_pid(&failed_run_id)).await;
        assert!(
            fs::read_to_string(&restores).unwrap_or_default().is_empty(),
            "a fresh run must not restore a checkpoint"
        );
        fs::write(
            fixture.repository.join(AFTER_FAILURE),
            "edited after the failure\n",
        )
        .assert_value_with("edit the retained workspace");
        let siblings = workspace_siblings(&fixture.repository);
        Self {
            fixture,
            restores,
            failed_run_id,
            siblings,
        }
    }

    async fn worker_checkpoint(&self) -> String {
        let catalog = self
            .fixture
            .json(&["checkpoints", &self.failed_run_id], "finish")
            .await;
        catalog
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

    fn control(&self, name: &str) -> PathBuf {
        self.restores.with_file_name(name)
    }

    /// Waits for the delayed restore number `count` and returns its wrapper, controller and sleep.
    async fn restore_processes(&self, count: usize) -> [u32; 3] {
        let deadline = Instant::now() + SLOW_RESUME_TIMEOUT;
        loop {
            let recorded = fs::read_to_string(self.control("pids")).unwrap_or_default();
            if let Some(line) = recorded.lines().nth(count - 1) {
                let pids = line
                    .split_whitespace()
                    .map(|pid| pid.parse::<u32>().assert_value_with("restore process PID"))
                    .collect::<Vec<_>>();
                return pids
                    .try_into()
                    .unwrap_or_else(|pids| panic!("malformed restore processes {pids:?}"));
            }
            assert!(
                Instant::now() < deadline,
                "delayed restore {count} never started"
            );
            sleep(Duration::from_millis(20)).await;
        }
    }

    /// An interruption before replacement leaves the checkout and its parent as they were.
    fn assert_checkout_untouched(&self) {
        assert!(
            self.fixture.repository.join(AFTER_FAILURE).exists(),
            "an interrupted extraction changed the checkout"
        );
        assert_eq!(
            workspace_siblings(&self.fixture.repository),
            self.siblings,
            "an interrupted extraction left its stage beside the workspace"
        );
    }

    async fn resume(&self, selection: &[&str]) -> String {
        let mut arguments = vec!["resume", self.failed_run_id.as_str()];
        arguments.extend_from_slice(selection);
        let output = timeout(
            SLOW_RESUME_TIMEOUT,
            self.fixture.command(&arguments, "finish").output(),
        )
        .await
        .assert_value_with("slow resume deadline")
        .assert_value_with("start slow resume");
        assert_success(
            &output,
            &format!("resume whose checkpoint restore takes {RESTORE_DELAY_SECONDS}s"),
        );
        let receipt: Value =
            serde_json::from_slice(&output.stdout).assert_value_with("resume receipt JSON");
        assert_eq!(
            receipt.assert_key("resumedFrom"),
            self.failed_run_id.as_str()
        );
        receipt
            .assert_key("runId")
            .as_str()
            .assert_value_with("successor run identity")
            .to_owned()
    }

    async fn assert_restored_and_succeeded(&self, successor: &str) {
        let status = self.fixture.wait_succeeded(successor, "finish").await;
        assert_eq!(
            status
                .assert_key("workspaceRecovery")
                .assert_key("resumedFrom"),
            self.failed_run_id.as_str()
        );
        assert!(
            !fs::read_to_string(&self.restores)
                .unwrap_or_default()
                .is_empty(),
            "the resume did not restore through the delayed Restic"
        );
        assert!(
            !self.fixture.repository.join(AFTER_FAILURE).exists(),
            "the successor ran without restoring the checkpointed workspace"
        );
        // Restore staging lives beside the workspace; a settled successor leaves none behind.
        wait_for_exit(self.fixture.ready_pid(successor)).await;
        assert_eq!(
            workspace_siblings(&self.fixture.repository),
            self.siblings,
            "restore staging remained beside the workspace"
        );
    }
}

fn workspace_siblings(workspace: &Path) -> Vec<std::ffi::OsString> {
    let mut siblings = fs::read_dir(workspace.parent().assert_value_with("workspace parent"))
        .assert_value_with("list the workspace parent")
        .map(|entry| entry.assert_value_with("workspace sibling").file_name())
        .collect::<Vec<_>>();
    siblings.sort();
    siblings
}
