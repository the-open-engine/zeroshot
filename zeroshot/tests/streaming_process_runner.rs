use std::collections::BTreeMap;
use std::fs;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::process::Stdio;

use tokio::sync::watch;
use tokio::time::{Duration, Instant, timeout};
use zeroshot_engine::execution::WorkspaceAccessMode;
use zeroshot_engine::execution::driver::WorkspaceCapability;
use zeroshot_engine::execution::process::{
    LocalProcessRunner, ProcessCleanupEvidence, ProcessSession, ProcessSessionCommand,
};
#[cfg(unix)]
use zeroshot_engine::execution::process::{
    MAX_PROCESS_DIAGNOSTIC_BYTES, MAX_PROCESS_FRAME_BYTES, MAX_PROCESS_FRAMING_OVERHEAD_BYTES,
    MAX_PROCESS_MESSAGE_BYTES, PROCESS_STDIN_CAPACITY, PROCESS_STDOUT_CAPACITY, ProcessFrame,
    ProcessLaunchEvidence, ProcessRunnerError,
};
#[path = "support/process_runner.rs"]
mod process_runner_support;
use process_runner_support::{
    cancellation_pair, process_exists, unique_temp_path, wait_for_child_pid, wait_for_process_exit,
};
#[cfg(unix)]
use process_runner_support::shell_quote;

fn command(program: &str, argv: Vec<&str>) -> ProcessSessionCommand {
    ProcessSessionCommand {
        program: program.to_owned(),
        argv: argv.into_iter().map(str::to_owned).collect(),
        environment: BTreeMap::new(),
        workspace: WorkspaceCapability {
            current_dir: std::env::temp_dir(),
            mode: WorkspaceAccessMode::ReadWrite,
        },
        deadline: Some(Instant::now() + Duration::from_secs(10)),
    }
}

#[test]
fn process_command_debug_exposes_environment_names_but_not_values() {
    let mut command = command("provider", vec![]);
    command.environment.insert(
        "OPENAI_API_KEY".to_owned(),
        "sensitive-provider-secret".to_owned(),
    );

    let debug = format!("{command:?}");
    assert!(debug.contains("OPENAI_API_KEY"));
    assert!(!debug.contains("sensitive-provider-secret"));
}

#[cfg(unix)]
async fn open(program: &str, argv: Vec<&str>) -> (watch::Sender<bool>, ProcessSession) {
    let (cancel, cancellation) = cancellation_pair();
    let session = LocalProcessRunner::new()
        .open(command(program, argv), cancellation)
        .await
        .assert_value();
    (cancel, session)
}

#[cfg(unix)]
async fn collect_stdout(session: &mut ProcessSession) -> Vec<u8> {
    let mut output = Vec::new();
    while let Some(chunk) = session.recv_stdout().await {
        output.extend_from_slice(chunk.as_slice());
    }
    output
}

#[cfg(unix)]
#[tokio::test]
async fn stdout_queue_applies_capacity_sixty_four_backpressure() {
    assert_eq!(PROCESS_STDOUT_CAPACITY, 64);
    assert_eq!(PROCESS_STDIN_CAPACITY, 64);
    let marker = unique_temp_path("zeroshot-stream-backpressure");
    let script = format!(
        "/usr/bin/head -c 7000000 /dev/zero; printf ready > {}; sleep 30",
        shell_quote(marker.to_string_lossy().as_ref())
    );
    let (_cancel, mut session) = open("/bin/sh", vec!["-c", &script]).await;
    assert_eq!(session.stdout_queue_capacity(), 64);
    assert_eq!(session.stdin_queue_capacity(), 64);

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !marker.exists(),
        "child passed the bounded stdout queue without a receiver"
    );
    let released = timeout(Duration::from_secs(4), session.release())
        .await
        .assert_value_with("release must not depend on stdout capacity")
        .assert_value();
    assert_eq!(released.cleanup, ProcessCleanupEvidence::Reaped);
    assert_eq!(released.post_launch_error, None);
    let _ = fs::remove_file(marker);
}

#[cfg(unix)]
#[tokio::test]
async fn duplex_preserves_split_frames_and_eof_with_pending_output() {
    let (_cancel, mut session) = open("/bin/cat", vec![]).await;
    let prefix = vec![b'a'; 63 * 1024];
    let boundary = vec![b'b'; 3 * 1024];
    let suffix = vec![b'c'; 512 * 1024];
    session
        .send(ProcessFrame::new(prefix.clone()).assert_value())
        .await
        .assert_value();
    session
        .send(ProcessFrame::new(boundary.clone()).assert_value())
        .await
        .assert_value();
    session
        .send(ProcessFrame::new(suffix.clone()).assert_value())
        .await
        .assert_value();
    session.close_stdin().await.assert_value();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let output = collect_stdout(&mut session).await;
    let mut expected = prefix;
    expected.extend_from_slice(&boundary);
    expected.extend_from_slice(&suffix);
    assert_eq!(output, expected);
    let completion = session.wait().await.assert_value();
    assert_eq!(completion.exit_code, Some(0));
    assert_eq!(completion.post_launch_error, None);
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_and_child_exit_races_settle_once() {
    for _ in 0..8 {
        let (cancel, mut session) =
            open("/bin/sh", vec!["-c", "printf ready; /bin/sleep 0.01"]).await;
        assert_eq!(
            session.recv_stdout().await.assert_value().as_slice(),
            b"ready"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        let _ = cancel.send(true);
        let completion = timeout(Duration::from_secs(2), session.wait())
            .await
            .assert_value_with("child-exit race must settle")
            .assert_value();
        if completion.cancelled {
            assert!(completion.cleanup.proves_tree_empty());
        } else {
            assert_eq!(completion.exit_code, Some(0));
        }
        assert_eq!(completion, session.release().await.assert_value());
    }

    let (cancel, mut session) = open("/bin/sleep", vec!["30"]).await;
    cancel.send(true).assert_value();
    let completion = timeout(Duration::from_secs(2), session.wait())
        .await
        .assert_value_with("cancellation must settle")
        .assert_value();
    assert!(completion.cancelled);
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
}

#[cfg(unix)]
#[tokio::test]
async fn deadline_force_kills_and_reaps_the_session() {
    let (_cancel, cancellation) = cancellation_pair();
    let mut timed = command("/bin/sleep", vec!["30"]);
    timed.deadline = Some(Instant::now() + Duration::from_millis(50));
    let mut session = LocalProcessRunner::new()
        .open(timed, cancellation)
        .await
        .assert_value();
    let completion = timeout(Duration::from_secs(2), session.wait())
        .await
        .assert_value_with("deadline must settle")
        .assert_value();
    assert!(completion.timed_out);
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
}

#[cfg(unix)]
#[tokio::test]
async fn child_death_closes_stream_and_reports_may_have_started() {
    let (_cancel, mut session) = open("/bin/sh", vec!["-c", "kill -9 $$"]).await;
    assert_eq!(session.recv_stdout().await, None);
    let completion = timeout(Duration::from_secs(2), session.wait())
        .await
        .assert_value()
        .assert_value();
    assert_eq!(
        completion.launch_evidence,
        ProcessLaunchEvidence::MayHaveStarted
    );
    assert_eq!(completion.exit_code, None);

    let (cancel, cancellation) = cancellation_pair();
    let _keep_sender_alive = cancel;
    let error = LocalProcessRunner::new()
        .open(command("/definitely/missing", vec![]), cancellation)
        .await
        .assert_error_with("missing process unexpectedly started");
    assert!(matches!(error, ProcessRunnerError::Launch(_)));
    assert_eq!(
        error.launch_evidence(),
        ProcessLaunchEvidence::DefinitelyNotStarted
    );
}

#[cfg(unix)]
#[tokio::test]
async fn release_drain_timeout_force_kills_and_reaps_descendants() {
    let pid_file = unique_temp_path("zeroshot-stream-descendant-pid");
    let script = format!(
        "sleep 30 & child=$!; printf %s \"$child\" > {}; wait",
        shell_quote(pid_file.to_string_lossy().as_ref())
    );
    let (_cancel, mut session) = open("/bin/sh", vec!["-c", &script]).await;
    let child_pid = wait_for_child_pid(&pid_file).await;
    assert!(process_exists(child_pid));

    let first = timeout(Duration::from_secs(4), session.release())
        .await
        .assert_value_with("release must force a non-draining child")
        .assert_value();
    let second = session.release().await.assert_value();
    assert_eq!(first, second);
    assert_eq!(first.cleanup, ProcessCleanupEvidence::Reaped);
    wait_for_process_exit(child_pid).await;
    let _ = fs::remove_file(pid_file);
}

use openengine_cluster_testkit::assertions::AssertValue;
#[cfg(unix)]
use openengine_cluster_testkit::assertions::AssertError;

#[cfg(unix)]
#[tokio::test]
async fn root_exit_and_cancel_race_still_reap_surviving_descendants() {
    let pid_file = unique_temp_path("zeroshot-stream-orphan-pid");
    let ready_file = unique_temp_path("zeroshot-stream-orphan-ready");
    let ready_path = shell_quote(ready_file.to_string_lossy().as_ref());
    let script = format!(
        "(printf ready > {ready_path}; exec /bin/sleep 30) </dev/null >/dev/null 2>&1 & child=$!; while [ ! -f {ready_path} ]; do :; done; printf %s \"$child\" > {}; exit 0",
        shell_quote(pid_file.to_string_lossy().as_ref())
    );
    let (_cancel, mut session) = open("/bin/sh", vec!["-c", &script]).await;
    let child_pid = wait_for_child_pid(&pid_file).await;
    assert_eq!(fs::read(&ready_file).assert_value(), b"ready");
    let completion = timeout(Duration::from_secs(4), session.wait())
        .await
        .assert_value_with("root exit must retain descendant cleanup ownership")
        .assert_value();
    assert_eq!(completion.exit_code, Some(0));
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
    assert_eq!(completion.post_launch_error, None);
    wait_for_process_exit(child_pid).await;
    let _ = fs::remove_file(&pid_file);
    let _ = fs::remove_file(&ready_file);

    let (cancel, mut session) = open("/bin/sh", vec!["-c", &script]).await;
    let child_pid = wait_for_child_pid(&pid_file).await;
    assert_eq!(fs::read(&ready_file).assert_value(), b"ready");
    let _ = cancel.send(true);
    let completion = timeout(Duration::from_secs(4), session.wait())
        .await
        .assert_value_with("root-exit cancellation race must retain cleanup ownership")
        .assert_value();
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
    assert_eq!(completion.post_launch_error, None);
    wait_for_process_exit(child_pid).await;
    let _ = fs::remove_file(pid_file);
    let _ = fs::remove_file(ready_file);
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_oversized_messages_and_classifies_post_launch_io() {
    assert_eq!(
        MAX_PROCESS_FRAME_BYTES,
        MAX_PROCESS_MESSAGE_BYTES + MAX_PROCESS_FRAMING_OVERHEAD_BYTES
    );
    assert!(
        ProcessFrame::with_framing(vec![0; MAX_PROCESS_FRAME_BYTES], MAX_PROCESS_MESSAGE_BYTES)
            .is_ok()
    );
    let error = ProcessFrame::new(vec![0; MAX_PROCESS_MESSAGE_BYTES + 1]).assert_error();
    assert!(matches!(error, ProcessRunnerError::InvalidCommand(_)));
    let overhead_error = ProcessFrame::with_framing(
        vec![0; MAX_PROCESS_FRAME_BYTES + 1],
        MAX_PROCESS_MESSAGE_BYTES,
    )
    .assert_error();
    assert!(matches!(
        overhead_error,
        ProcessRunnerError::InvalidCommand(_)
    ));
    assert_eq!(
        error.launch_evidence(),
        ProcessLaunchEvidence::DefinitelyNotStarted
    );

    let (_cancel, mut session) = open("/bin/true", vec![]).await;
    session.wait().await.assert_value();
    let error = session
        .send(ProcessFrame::new(b"late".to_vec()).assert_value())
        .await
        .assert_error();
    assert!(matches!(error, ProcessRunnerError::Io(_)));
    assert_eq!(
        error.launch_evidence(),
        ProcessLaunchEvidence::MayHaveStarted
    );
}

#[cfg(unix)]
#[tokio::test]
async fn stderr_keeps_only_the_bounded_diagnostic_tail() {
    let script =
        "i=0; while [ \"$i\" -lt 70000 ]; do printf a 1>&2; i=$((i+1)); done; printf TAIL 1>&2";
    let (_cancel, mut session) = open("/bin/sh", vec!["-c", script]).await;
    let completion = timeout(Duration::from_secs(5), session.wait())
        .await
        .assert_value()
        .assert_value();
    assert_eq!(completion.stderr_tail.len(), MAX_PROCESS_DIAGNOSTIC_BYTES);
    assert!(completion.stderr_tail.ends_with(b"TAIL"));
    assert!(completion.stderr_tail_truncated);
    assert_eq!(completion.post_launch_error, None);
}

#[cfg(unix)]
#[tokio::test]
async fn close_and_release_are_idempotent_for_one_shot_sessions() {
    let (_cancel, mut session) = open("/bin/cat", vec![]).await;
    session
        .send(ProcessFrame::new(b"one-shot\n".to_vec()).assert_value())
        .await
        .assert_value();
    session.close_stdin().await.assert_value();
    session.close_stdin().await.assert_value();
    assert_eq!(collect_stdout(&mut session).await, b"one-shot\n");

    let waited = session.wait().await.assert_value();
    let first_release = session.release().await.assert_value();
    let second_release = session.release().await.assert_value();
    assert_eq!(waited, first_release);
    assert_eq!(first_release, second_release);
    assert_eq!(
        first_release.cleanup,
        if cfg!(target_os = "linux") {
            ProcessCleanupEvidence::Reaped
        } else {
            ProcessCleanupEvidence::NotRequired
        }
    );
}

#[cfg(windows)]
const WINDOWS_PROCESS_FIXTURE: &str = "ZEROSHOT_STREAMING_PROCESS_FIXTURE";

#[cfg(windows)]
async fn windows_process_fixture(pid_file: std::ffi::OsString) {
    if pid_file == "descendant" {
        tokio::time::sleep(Duration::from_secs(60)).await;
        return;
    }
    let mut child = tokio::process::Command::new(std::env::current_exe().assert_value())
        .args(["--exact", "windows_job_release_reaps_a_descendant"])
        .env(WINDOWS_PROCESS_FIXTURE, "descendant")
        .kill_on_drop(true)
        .spawn()
        .assert_value();
    let pid_file = PathBuf::from(pid_file);
    fs::write(
        pid_file.with_extension("root.pid"),
        std::process::id().to_string(),
    )
    .assert_value();
    fs::write(&pid_file, child.id().assert_value().to_string()).assert_value();
    if std::env::var_os("ZEROSHOT_STREAMING_ROOT_EXIT").is_some() {
        // Skip Rust destructors so kill_on_drop cannot accidentally supply the cleanup proof.
        std::process::exit(37);
    }
    child.wait().await.assert_value();
}

#[cfg(windows)]
async fn windows_owned_session(
    pid_file: &std::path::Path,
    root_exits: bool,
) -> (watch::Sender<bool>, ProcessSession) {
    let executable = std::env::current_exe().assert_value();
    let mut launch = command(
        executable.to_str().assert_value(),
        vec!["--exact", "windows_job_release_reaps_a_descendant"],
    );
    launch.environment.insert(
        "SystemRoot".to_owned(),
        std::env::var("SystemRoot").assert_value(),
    );
    launch.environment.insert(
        WINDOWS_PROCESS_FIXTURE.to_owned(),
        pid_file.to_str().assert_value().to_owned(),
    );
    launch.deadline = None;
    if root_exits {
        launch
            .environment
            .insert("ZEROSHOT_STREAMING_ROOT_EXIT".to_owned(), "1".to_owned());
    }
    let (cancel, cancellation) = cancellation_pair();
    let mut session = LocalProcessRunner::new()
        .open(launch, cancellation)
        .await
        .assert_value();
    session.close_stdin().await.assert_value();
    (cancel, session)
}

#[cfg(windows)]
#[tokio::test]
async fn windows_job_release_reaps_a_descendant() {
    if let Some(pid_file) = std::env::var_os(WINDOWS_PROCESS_FIXTURE) {
        windows_process_fixture(pid_file).await;
        return;
    }
    let pid_file = unique_temp_path("zeroshot-stream-windows-descendant-pid");
    let (cancel, mut session) = windows_owned_session(&pid_file, false).await;
    let child_pid = tokio::select! {
        pid = wait_for_child_pid(&pid_file) => pid,
        output = session.wait() => {
            let output = output.assert_value();
            panic!(
                "fixture exited before recording its child (exit {:?}): {}",
                output.exit_code,
                String::from_utf8_lossy(&output.stderr_tail),
            );
        }
    };
    assert!(process_exists(child_pid));
    cancel.send(true).assert_value();
    let completion = timeout(Duration::from_secs(5), session.wait())
        .await
        .assert_value()
        .assert_value();
    assert!(completion.cancelled);
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
    wait_for_process_exit(child_pid).await;
    let _ = fs::remove_file(pid_file);
}

#[cfg(windows)]
#[tokio::test]
async fn windows_job_root_exit_reaps_a_descendant() {
    let root = tempfile::tempdir().assert_value();
    let pid_file = root.path().join("child.pid");
    let (_cancel, mut session) = windows_owned_session(&pid_file, true).await;
    let child = wait_for_child_pid(&pid_file).await;
    let output = timeout(Duration::from_secs(5), session.wait())
        .await
        .assert_value()
        .assert_value();
    assert_eq!(output.exit_code, Some(37));
    assert_eq!(output.cleanup, ProcessCleanupEvidence::Reaped);
    wait_for_process_exit(child).await;
}

#[cfg(windows)]
#[tokio::test]
async fn windows_job_owner_death_reaps_only_its_descendants() {
    const OWNER: &str = "ZEROSHOT_STREAMING_OWNER_FIXTURE";
    if let Some(root) = std::env::var_os(OWNER) {
        let root = PathBuf::from(root);
        let _owned = windows_owned_session(&root.join("owner-child.pid"), false).await;
        std::future::pending::<()>().await;
        return;
    }
    let root = tempfile::tempdir().assert_value();
    let peer_pid_file = root.path().join("peer-child.pid");
    let (peer_cancel, mut peer) = windows_owned_session(&peer_pid_file, false).await;
    let peer_child = wait_for_child_pid(&peer_pid_file).await;
    // The owner is an ordinary process, outside the peer's Job. Its abrupt death must close
    // its own Job handle; an outer managed Job here would mask a leaked inner handle.
    let mut owner_command = tokio::process::Command::new(std::env::current_exe().assert_value());
    owner_command
        .args([
            "--exact",
            "windows_job_owner_death_reaps_only_its_descendants",
        ])
        .env(OWNER, root.path())
        .env_remove(WINDOWS_PROCESS_FIXTURE)
        .env_remove("ZEROSHOT_STREAMING_ROOT_EXIT");
    owner_command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut owner = owner_command.kill_on_drop(true).spawn().assert_value();
    let owned_pid_file = root.path().join("owner-child.pid");
    let owned_child = wait_for_child_pid(&owned_pid_file).await;
    let owned_root = wait_for_child_pid(&owned_pid_file.with_extension("root.pid")).await;
    assert!(
        process_exists(owned_root) && process_exists(owned_child) && process_exists(peer_child)
    );
    owner.start_kill().assert_value();
    timeout(Duration::from_secs(5), owner.wait())
        .await
        .assert_value()
        .assert_value();
    wait_for_process_exit(owned_root).await;
    wait_for_process_exit(owned_child).await;
    assert!(
        process_exists(peer_child),
        "owner death killed an unrelated Job's descendant"
    );
    peer_cancel.send(true).assert_value();
    let output = timeout(Duration::from_secs(5), peer.wait())
        .await
        .assert_value()
        .assert_value();
    assert_eq!(output.cleanup, ProcessCleanupEvidence::Reaped);
    wait_for_process_exit(peer_child).await;
}

#[cfg(unix)]
#[tokio::test]
async fn unbounded_process_remains_cancellable_and_reaped() {
    let (cancel, cancellation) = cancellation_pair();
    let mut request = command("/bin/sleep", vec!["30"]);
    request.deadline = None;
    let mut session = LocalProcessRunner::new()
        .open(request, cancellation)
        .await
        .assert_value();
    assert!(
        timeout(Duration::from_millis(30), session.wait())
            .await
            .is_err()
    );
    cancel.send(true).assert_value();
    let completion = timeout(Duration::from_secs(2), session.wait())
        .await
        .assert_value()
        .assert_value();
    assert!(completion.cancelled);
    assert!(!completion.timed_out);
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
}

#[cfg(target_os = "linux")]
#[path = "support/process_tree_ownership.rs"]
mod tree_ownership;

#[cfg(target_os = "linux")]
#[tokio::test]
async fn descendants_that_leave_the_process_group_stop_before_settlement() {
    let mut violations = Vec::new();
    for escape in ["setsid", "setpgid", "double-fork"] {
        for finish in ["exit", "cancel", "deadline"] {
            let tool = tree_ownership::EscapedTool::new();
            let root = r#"import os,sys,subprocess,time
subprocess.Popen([sys.executable,sys.argv[1],sys.argv[2],sys.argv[3]])
while not os.path.exists(sys.argv[3]+'/tool.pid'): time.sleep(.01)
if sys.argv[4]=='exit': sys.exit(0)
time.sleep(60)
"#;
            let args = vec![
                "-c".to_owned(),
                root.to_owned(),
                tool.script.to_string_lossy().into_owned(),
                escape.to_owned(),
                tool.directory().to_string_lossy().into_owned(),
                finish.to_owned(),
            ];
            let mut request = command(&tool.python, vec![]);
            request.argv = args;
            request.deadline =
                (finish == "deadline").then(|| Instant::now() + Duration::from_secs(2));
            let (cancel, cancellation) = cancellation_pair();
            let mut session = LocalProcessRunner::new()
                .open(request, cancellation)
                .await
                .assert_value();
            tool.wait_started().await;
            if finish == "cancel" {
                cancel.send_replace(true);
            }
            let result = timeout(Duration::from_secs(4), async {
                collect_stdout(&mut session).await;
                session.wait().await
            })
            .await;
            if result.is_err() {
                violations.push(format!(
                    "{escape}/{finish}: settlement waits for escaped stdout"
                ));
            }
            let _ = session.release().await;
            if tool.live() {
                violations.push(format!("{escape}/{finish}: writer outlived settlement"));
            }
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}
