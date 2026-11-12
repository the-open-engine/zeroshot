//! Restore checkpoint-only delivery programs before the one forge admission request.

use super::*;
use openengine_cluster_testkit::assertions::JsonAt;

const PROJECT_READ: &str = "checkpoint-project-read";
const PROXY: &str = "http://caller:checkpoint-proxy-secret@127.0.0.1:9";
/// While this file exists, the fixture `gh` records its processes and never answers.
const STALLED_READ: &str = "stall-project-read";
const STALLED_READ_PROCESSES: &str = "stalled-project-read-processes";

/// A failed `--pr` run whose worker checkpoint carries the only `gh`; the workspace copy is gone.
struct CheckpointedRun {
    path: OsString,
    tools: PathBuf,
    run_id: String,
    checkpoint: String,
    original_turns: Vec<u8>,
}

async fn checkpointed_run(fixture: &DeliveryFixture) -> CheckpointedRun {
    let tools = fixture.repository.join("tools");
    fs::create_dir(&tools).assert_value();
    write_checkpoint_gh(fixture, &tools.join("gh"));
    git(&fixture.repository, &["add", "tools/gh"]);
    git(
        &fixture.repository,
        &["commit", "-m", "carry delivery executable"],
    );
    // Exclude system gh while retaining the host programs the fake harness and checkpoints need.
    let host = fixture.root.path("host-tools");
    fs::create_dir(&host).assert_value();
    for program in ["cat", "grep"] {
        std::os::unix::fs::symlink(host_executable(program), host.join(program)).assert_value();
    }
    let sibling = Path::new(env!("CARGO_BIN_EXE_zeroshot")).with_file_name("restic");
    let restic = if sibling.is_file() {
        sibling
    } else {
        host_executable("restic")
    };
    std::os::unix::fs::symlink(restic, host.join("restic")).assert_value();
    let path = search_path(&[PathBuf::from("tools"), fixture.bin.clone(), host]);
    let run = fixture.run("--pr", &path).await;
    assert!(
        !run.status.success() && fixture.codex_log.exists(),
        "precondition: the fake delivery leaves a failed run with worker checkpoints: {run:?}"
    );
    let run_id = receipt_id(&run);
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
    fs::remove_file(tools.join("gh")).assert_value();
    CheckpointedRun {
        path,
        tools,
        run_id,
        checkpoint,
        original_turns,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_resume_checks_the_restored_gh_once_before_agents_start() {
    let fixture = DeliveryFixture::new();
    let CheckpointedRun {
        path,
        tools,
        run_id,
        checkpoint,
        original_turns,
    } = checkpointed_run(&fixture).await;
    let checkpoint = checkpoint.as_str();
    fs::write(fixture.root.path("check-resume-network"), b"check").assert_value();

    let plain = output(&fixture, &["resume", &run_id], &path).await;
    assert!(
        !plain.status.success()
            && String::from_utf8_lossy(&plain.stderr).contains("needs the `gh` executable"),
        "plain resume must still check the retained workspace: {plain:?}"
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
    let untrusted = timeout(
        CLI_TIMEOUT,
        fixture
            .command(&path)
            .args(arguments)
            .env("SSL_CERT_FILE", &certificate)
            .output(),
    )
    .await
    .assert_value()
    .assert_value();
    assert!(
        !untrusted.status.success()
            && String::from_utf8_lossy(&untrusted.stderr)
                .contains(certificate.to_str().assert_value()),
        "checkpoint refusal must name the unreadable CA file: {untrusted:?}"
    );
    assert!(tools.join("gh").is_file(), "gh was not restored");
    assert_eq!(
        project_reads(&fixture),
        1,
        "unreadable CA must prevent the request"
    );
    assert_eq!(fs::read(&fixture.codex_log).assert_value(), original_turns);
    fs::remove_file(tools.join("gh")).assert_value();
    let rejected = timeout(
        CLI_TIMEOUT,
        fixture
            .command(&path)
            .args(arguments)
            .env("GH_TOKEN", "rejected-checkpoint-token")
            .env("HTTPS_PROXY", PROXY)
            .env("NO_PROXY", "caller-bypass.invalid")
            .output(),
    )
    .await
    .assert_value()
    .assert_value();
    let reason = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        !rejected.status.success()
            && rejected.stdout.is_empty()
            && reason.contains("authenticated project/version read")
            && reason.contains("HTTP 401"),
        "restored gh must refuse authentication before agents start: {rejected:?}"
    );
    for secret in [
        PROXY,
        "checkpoint-proxy-secret",
        "rejected-checkpoint-token",
    ] {
        assert!(!reason.contains(secret), "secret in refusal: {reason}");
    }
    assert!(tools.join("gh").is_file(), "gh was not restored");
    assert_eq!(
        project_reads(&fixture),
        2,
        "one request per attempted resume"
    );
    assert_eq!(fs::read(&fixture.codex_log).assert_value(), original_turns);
    let status = json_output(&fixture, &["status", &run_id], &path).await;
    let recovery = status.assert_key("workspaceRecovery");
    assert_eq!(recovery.assert_key("recoverable"), true);
    assert!(recovery.get("successorRunId").is_none(), "{recovery}");
    let listed = json_output(&fixture, &["list"], &path).await;
    assert_eq!(listed.assert_key("runs").as_array().assert_value().len(), 1);

    // The refused restore retains its checkpoint and releases the claim for a corrected retry.
    fs::remove_file(tools.join("gh")).assert_value();
    let resumed = output(&fixture, &arguments, &path).await;
    assert!(resumed.status.success(), "{resumed:?}");
    let successor = receipt_id(&resumed);
    let watched = output(&fixture, &["watch", &successor], &path).await;
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
    assert!(tools.join("gh").is_file());
}

/// The restored check is startup work: the controller's stop request must end it, clean up the
/// `gh` process tree and report the startup failure itself, well inside its shutdown deadline.
#[tokio::test(flavor = "multi_thread")]
async fn controller_shutdown_cancels_the_restored_forge_check_before_agents_start() {
    let fixture = DeliveryFixture::new();
    let CheckpointedRun {
        path,
        tools,
        run_id,
        checkpoint,
        original_turns,
    } = checkpointed_run(&fixture).await;
    let processes = fixture.root.path(STALLED_READ_PROCESSES);
    fs::write(fixture.root.path(STALLED_READ), b"stall").assert_value();

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
    let (controller, stalled) = timeout(CLI_TIMEOUT, async {
        loop {
            let recorded = fs::read_to_string(&processes).unwrap_or_default();
            let stalled = recorded
                .split_whitespace()
                .filter_map(|process| process.parse::<u32>().ok())
                .collect::<Vec<_>>();
            if let (Some(controller), 2) = (detached_controller(cli), stalled.len()) {
                break (controller, stalled);
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
    for process in stalled {
        assert!(
            !process_exists(process),
            "startup failure was reported before gh process {process} was cleaned up"
        );
    }
    assert!(tools.join("gh").is_file(), "gh was not restored");
    assert_eq!(project_reads(&fixture), 2, "the check must start once");
    assert_eq!(fs::read(&fixture.codex_log).assert_value(), original_turns);
    fs::remove_file(fixture.root.path(STALLED_READ)).assert_value();
    let status = json_output(&fixture, &["status", &run_id], &path).await;
    let recovery = status.assert_key("workspaceRecovery");
    assert_eq!(recovery.assert_key("recoverable"), true);
    assert!(recovery.get("successorRunId").is_none(), "{recovery}");
    let listed = json_output(&fixture, &["list"], &path).await;
    assert_eq!(listed.assert_key("runs").as_array().assert_value().len(), 1);
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

fn process_exists(process: u32) -> bool {
    // A killed process that its parent has not reaped yet no longer runs.
    let zombie = fs::read_to_string(format!("/proc/{process}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, fields)| fields.starts_with('Z'))
    });
    unsafe { libc::kill(process as libc::pid_t, 0) == 0 && !zombie }
}

async fn output(fixture: &DeliveryFixture, arguments: &[&str], path: &OsString) -> Output {
    timeout(
        CLI_TIMEOUT,
        fixture
            .command(path)
            .args(arguments)
            .env("HTTPS_PROXY", PROXY)
            .env("NO_PROXY", "caller-bypass.invalid")
            .output(),
    )
    .await
    .assert_value()
    .assert_value()
}

async fn json_output(fixture: &DeliveryFixture, arguments: &[&str], path: &OsString) -> Value {
    let result = output(fixture, arguments, path).await;
    assert!(result.status.success(), "{result:?}");
    serde_json::from_slice(&result.stdout).assert_value()
}

fn receipt_id(result: &Output) -> String {
    let first = result
        .stdout
        .split(|byte| *byte == b'\n')
        .next()
        .assert_value();
    let receipt: Value = serde_json::from_slice(first).assert_value();
    receipt
        .assert_key("runId")
        .as_str()
        .assert_value()
        .to_owned()
}

fn project_reads(fixture: &DeliveryFixture) -> usize {
    fs::read_to_string(&fixture.gh_log)
        .assert_value()
        .lines()
        .filter(|line| *line == PROJECT_READ)
        .count()
}

fn write_checkpoint_gh(fixture: &DeliveryFixture, path: &Path) {
    openengine_cluster_testkit::fixture::write_executable(
        path,
        format!(
            r#"#!/bin/sh
if [ "$1" = api ] && [ "$2" = repos/acme/local-fixture ]; then
    printf '%s\n' '{PROJECT_READ}' >> '{log}'
    if [ -f '{stall}' ]; then
        /bin/sleep 60 &
        printf '%s %s\n' "$$" "$!" > '{processes}'
        wait
    fi
    if [ -f '{marker}' ]; then
        if [ "$HTTPS_PROXY" != '{PROXY}' ] || [ "$NO_PROXY" != caller-bypass.invalid ]; then
            printf '%s\n' 'captured network settings missing' >&2
            exit 1
        fi
    fi
    if [ "$GH_TOKEN" != fixture-github-token ]; then
        printf 'HTTP 401: rejected checkpoint token %s\n' "$HTTPS_PROXY" >&2
        exit 1
    fi
    printf 'HTTP/2 200 OK\r\n\r\n{{"full_name":"acme/local-fixture"}}'
    exit 0
fi
printf '%s\n' 'fixture gh returns no GitHub data'
"#,
            log = fixture.gh_log.display(),
            stall = fixture.root.path(STALLED_READ).display(),
            processes = fixture.root.path(STALLED_READ_PROCESSES).display(),
            marker = fixture.root.path("check-resume-network").display()
        ),
        0o755,
    )
    .assert_value();
}
