//! Restore the checkout’s forge certificate before its single admission request.

use super::*;
use openengine_cluster_testkit::assertions::JsonAt;

/// The worker checkpoint of a failed delivery run carries the removed checkout CA.
struct CheckpointedRun {
    path: OsString,
    ca: PathBuf,
    run_id: String,
    checkpoint: String,
    original_turns: Vec<u8>,
}

async fn checkpointed_run(fixture: &mut DeliveryFixture) -> CheckpointedRun {
    let ca = fixture.repository.join("tools/forge-ca.pem");
    fs::create_dir(ca.parent().assert_value()).assert_value();
    fs::copy(&fixture.root_ca, &ca).assert_value();
    fixture.ca_file = PathBuf::from("tools/forge-ca.pem");
    git(&fixture.repository, &["add", "tools/forge-ca.pem"]);
    git(&fixture.repository, &["commit", "-m", "Carry forge CA"]);
    let path = search_path(&[fixture.bin.clone(), fixture.link_host_tools()]);
    let run = fixture.run("--pr", &path).await;
    assert!(
        !run.status.success() && fixture.codex_log.exists(),
        "precondition: the fake delivery leaves a failed run with worker checkpoints: {run:?}"
    );
    let run_id = receipt_part_id(&run);
    let checkpoints = json_output(fixture, &["checkpoints", &run_id], &path).await;
    let checkpoint = checkpoints
        .assert_key("checkpoints")
        .as_array()
        .assert_value()
        .iter()
        .find(|entry| entry.assert_key("node") == "worker")
        .assert_value()
        .assert_key("checkpointId")
        .as_str()
        .assert_value()
        .to_owned();
    assert_eq!(project_reads(fixture), 1);
    let original_turns = fs::read(&fixture.codex_log).assert_value();
    fs::remove_file(&ca).assert_value();
    CheckpointedRun {
        path,
        ca,
        run_id,
        checkpoint,
        original_turns,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_resume_checks_the_restored_certificate_once_before_agents_start() {
    let mut fixture = DeliveryFixture::new().await;
    let CheckpointedRun {
        path,
        ca,
        run_id,
        checkpoint,
        original_turns,
    } = checkpointed_run(&mut fixture).await;
    let checkpoint = checkpoint.as_str();

    let plain = output(&fixture, &["resume", &run_id, "--restart"], &path).await;
    assert!(
        !plain.status.success()
            && String::from_utf8_lossy(&plain.stderr).contains("tools/forge-ca.pem"),
        "explicit restart must still check the retained workspace: {plain:?}"
    );
    assert_eq!(project_reads(&fixture), 1);

    let arguments = [
        "resume",
        &run_id,
        "--from-checkpoint",
        checkpoint,
        "--discard-workspace-changes",
    ];
    let certificate = fixture.root.path("missing-ca.pem");
    let mut untrusted = forge_command(&fixture, &arguments, &path);
    untrusted.env("SSL_CERT_FILE", &certificate);
    let untrusted = command_output(untrusted).await;
    assert!(
        !untrusted.status.success()
            && String::from_utf8_lossy(&untrusted.stderr)
                .contains(certificate.to_str().assert_value()),
        "checkpoint refusal must name the unreadable CA file: {untrusted:?}"
    );
    assert!(ca.is_file(), "CA was not restored");
    assert_eq!(
        project_reads(&fixture),
        1,
        "unreadable CA must prevent the request"
    );
    assert_eq!(fs::read(&fixture.codex_log).assert_value(), original_turns);
    fs::remove_file(&ca).assert_value();
    let mut rejected = forge_command(&fixture, &arguments, &path);
    rejected
        .env("GH_TOKEN", REJECTED_TOKEN)
        .env("GH_ENTERPRISE_TOKEN", REJECTED_TOKEN);
    let rejected = command_output(rejected).await;
    let reason = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        !rejected.status.success()
            && rejected.stdout.is_empty()
            && reason.contains("authenticated project/version read")
            && reason.contains("HTTP 401"),
        "forge must refuse authentication before agents start: {rejected:?}"
    );
    for secret in [PROXY, "checkpoint-proxy-secret", REJECTED_TOKEN] {
        assert!(!reason.contains(secret), "secret in refusal: {reason}");
    }
    assert!(ca.is_file(), "CA was not restored");
    assert_eq!(
        project_reads(&fixture),
        2,
        "one request per attempted resume"
    );
    assert_eq!(fs::read(&fixture.codex_log).assert_value(), original_turns);
    assert_no_successor(&fixture, &run_id, &path).await;

    // The refused restore retains its checkpoint and releases the claim for a corrected retry.
    fs::remove_file(&ca).assert_value();
    let resumed = output(&fixture, &arguments, &path).await;
    assert!(resumed.status.success(), "{resumed:?}");
    let successor = receipt_part_id(&resumed);
    assert_ne!(successor, run_id);
    let watched = output(&fixture, &["watch", &run_id, "--part", &successor], &path).await;
    assert!(
        !watched.status.success(),
        "fake delivery unexpectedly succeeded"
    );
    assert_eq!(
        project_reads(&fixture),
        3,
        "the controller duplicated preflight"
    );
    assert!(
        fs::read(&fixture.codex_log).assert_value().len() > original_turns.len(),
        "accepted checkpoint resume did not start agents"
    );
    assert!(ca.is_file());
}

/// A controller stop interrupts the restored HTTPS check and closes its socket before admission.
#[tokio::test(flavor = "multi_thread")]
async fn controller_shutdown_cancels_the_restored_forge_check_before_agents_start() {
    let mut fixture = DeliveryFixture::new().await;
    let CheckpointedRun {
        path,
        ca,
        run_id,
        checkpoint,
        original_turns,
    } = checkpointed_run(&mut fixture).await;
    fixture.forge.stall.store(true, Ordering::SeqCst);

    let child = fixture
        .command(&path)
        .args([
            "resume",
            &run_id,
            "--from-checkpoint",
            &checkpoint,
            "--discard-workspace-changes",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .assert_value();
    let cli = child.id().assert_value();
    let controller = timeout(CLI_TIMEOUT, async {
        loop {
            if let (Some(controller), 2) = (detached_controller(cli), project_reads(&fixture)) {
                break controller;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .assert_value_with("the restored forge check must start");
    assert_eq!(
        unsafe { libc::kill(controller as libc::pid_t, libc::SIGTERM) },
        0,
        "request the controller's stop"
    );

    let stopped = timeout(CLI_TIMEOUT, child.wait_with_output())
        .await
        .assert_value_with("the controller must report its own startup failure")
        .assert_value();
    let reason = String::from_utf8_lossy(&stopped.stderr);
    assert!(
        !stopped.status.success() && reason.contains("forge access check was cancelled"),
        "the stop request must cancel the forge check: {stopped:?}"
    );
    assert!(
        !reason.contains("cleanup could not be confirmed"),
        "{reason}"
    );
    assert!(
        stopped.stdout.is_empty(),
        "cancelled startup admitted a run"
    );
    await_disconnect(&fixture).await;
    assert!(ca.is_file(), "CA was not restored");
    assert_eq!(project_reads(&fixture), 2, "the check must start once");
    assert_eq!(fs::read(&fixture.codex_log).assert_value(), original_turns);
    assert_no_successor(&fixture, &run_id, &path).await;
}

/// An explicit restart asks the forge under its own recovery claim. The lock that every local
/// submission and resume on this state directory takes must stay free meanwhile.
#[tokio::test(flavor = "multi_thread")]
async fn stalled_plain_resume_forge_check_does_not_block_other_local_commands() {
    const UNBLOCKED: Duration = Duration::from_secs(20);
    let mut fixture = DeliveryFixture::new().await;
    let CheckpointedRun {
        path, ca, run_id, ..
    } = checkpointed_run(&mut fixture).await;
    fs::copy(&fixture.root_ca, &ca).assert_value();
    fixture.forge.stall.store(true, Ordering::SeqCst);

    let mut stalled = fixture
        .command(&path)
        .args(["resume", &run_id, "--restart"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .assert_value();
    timeout(CLI_TIMEOUT, async {
        while project_reads(&fixture) != 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .assert_value_with("the explicit restart's forge check must start");

    // The claim still excludes a second resume of this run, which status reports at once.
    let second = timeout(
        UNBLOCKED,
        output(&fixture, &["resume", &run_id, "--restart"], &path),
    )
    .await
    .assert_value_with("a second resume must not wait for the stalled forge request");
    assert!(
        !second.status.success()
            && String::from_utf8_lossy(&second.stderr).contains("already being resumed")
            && String::from_utf8_lossy(&second.stderr).contains(
                &*fixture
                    .state
                    .join("runs")
                    .join(&run_id)
                    .join("workspace-recovery.claim")
                    .to_string_lossy(),
            ),
        "the stalled resume must keep its recovery claim: {second:?}"
    );
    // A new submission takes the shared lock; it must be admitted while the request is stalled.
    let unrelated = fixture.root.path("unrelated-workspace");
    fs::create_dir(&unrelated).assert_value();
    let submission = [
        "--delivery",
        "none",
        "--cwd",
        unrelated.to_str().assert_value(),
        "--detach",
    ];
    let admitted = timeout(UNBLOCKED, fixture.run_arguments(&submission, &path))
        .await
        .assert_value_with("an unrelated run must not wait for the stalled forge request");
    assert!(
        admitted.status.success(),
        "an unrelated run without delivery must be admitted: {admitted:?}"
    );
    assert_eq!(
        project_reads(&fixture),
        2,
        "only the stalled resume may have asked the forge"
    );

    stalled.kill().await.assert_value();
    await_disconnect(&fixture).await;
    let status = json_output(&fixture, &["status", &run_id], &path).await;
    let recovery = status.assert_key("workspaceRecovery");
    assert_eq!(recovery.assert_key("recoverable"), true);
    assert!(recovery.get("successorRunId").is_none(), "{recovery}");
}

/// The foreground CLI starts the detached controller as its only child and waits for it.
fn detached_controller(cli: u32) -> Option<u32> {
    let children = StdCommand::new("pgrep")
        .args(["-P", &cli.to_string()])
        .output()
        .assert_value_with("pgrep lists the foreground CLI's children");
    let mut children = String::from_utf8_lossy(&children.stdout)
        .split_whitespace()
        .filter_map(|child| child.parse::<u32>().ok())
        .collect::<Vec<_>>();
    (children.len() == 1).then(|| children.remove(0))
}

async fn await_disconnect(fixture: &DeliveryFixture) {
    timeout(
        Duration::from_secs(5),
        fixture.forge.disconnected.notified(),
    )
    .await
    .assert_value_with("the client must close its stalled HTTPS socket");
}

fn forge_command(fixture: &DeliveryFixture, arguments: &[&str], path: &OsString) -> Command {
    let mut command = fixture.command(path);
    command
        .args(arguments)
        .env("HTTPS_PROXY", PROXY)
        .env("NO_PROXY", NO_PROXY);
    command
}

async fn command_output(mut command: Command) -> Output {
    timeout(CLI_TIMEOUT, command.output())
        .await
        .assert_value()
        .assert_value()
}

async fn output(fixture: &DeliveryFixture, arguments: &[&str], path: &OsString) -> Output {
    command_output(forge_command(fixture, arguments, path)).await
}

async fn assert_no_successor(fixture: &DeliveryFixture, run_id: &str, path: &OsString) {
    let status = json_output(fixture, &["status", run_id], path).await;
    let recovery = status.assert_key("workspaceRecovery");
    assert_eq!(recovery.assert_key("recoverable"), true);
    assert!(recovery.get("successorRunId").is_none(), "{recovery}");
    let listed = json_output(fixture, &["list"], path).await;
    assert_eq!(listed.assert_key("runs").as_array().assert_value().len(), 1);
}

async fn json_output(fixture: &DeliveryFixture, arguments: &[&str], path: &OsString) -> Value {
    let result = output(fixture, arguments, path).await;
    assert!(result.status.success(), "{result:?}");
    serde_json::from_slice(&result.stdout).assert_value()
}

fn receipt_part_id(result: &Output) -> String {
    let first = result
        .stdout
        .split(|byte| *byte == b'\n')
        .next()
        .assert_value();
    let receipt: Value = serde_json::from_slice(first).assert_value();
    receipt
        .assert_key("currentPartId")
        .as_str()
        .assert_value()
        .to_owned()
}

fn project_reads(fixture: &DeliveryFixture) -> usize {
    fixture.forge.project_reads.load(Ordering::SeqCst)
}
