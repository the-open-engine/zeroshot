use std::fs;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

use openengine_cluster_testkit::assertions::AssertValue;
#[cfg(unix)]
use openengine_cluster_testkit::assertions::AssertError;

#[cfg(unix)]
use super::session_io::PROCESS_OUTPUT_CHUNK_BYTES;
use super::write_new_file;
#[cfg(target_os = "linux")]
use super::{HostedProcessPool, HostedProcessScope};
#[cfg(unix)]
use super::{
    LocalProcessRunner, PROCESS_STDOUT_CAPACITY, ProcessCleanupEvidence, ProcessSession,
    ProcessRunnerError, ProcessSessionCommand, ProcessSessionOutput, prepare_local_private_home,
};
#[cfg(unix)]
use crate::execution::WorkspaceAccessMode;
#[cfg(unix)]
use crate::execution::driver::{DriverCancellation, WorkspaceCapability};
use crate::native_v2_candidate::test_support::TestDirectory;

#[test]
#[cfg(target_os = "linux")]
fn hosted_scopes_keep_loop_sessions_stable_and_executions_disjoint() {
    let pool = HostedProcessPool::new(10_002, 10_002, 20_000).assert_value();
    let loop_scope = HostedProcessScope::VerifierNodeInstance(7);
    let repeated = pool.identity(loop_scope).assert_value();
    let first_execution = pool
        .identity(HostedProcessScope::VerifierExecution(7))
        .assert_value();
    let second_execution = pool
        .identity(HostedProcessScope::VerifierExecution(8))
        .assert_value();

    assert_eq!(
        pool.identity(loop_scope).assert_value().uid(),
        repeated.uid()
    );
    assert_eq!(repeated.uid(), first_execution.uid());
    assert_ne!(
        repeated.runner().containment.membership(),
        first_execution.runner().containment.membership()
    );
    assert_eq!(first_execution.uid(), second_execution.uid());
    assert_ne!(
        first_execution.runner().containment.membership(),
        second_execution.runner().containment.membership()
    );
    assert_eq!(
        loop_scope.private_home(Path::new("/runtime")),
        Path::new("/runtime/verifier-node-instance-7")
    );
    assert_eq!(
        HostedProcessScope::VerifierExecution(7).private_home(Path::new("/runtime")),
        Path::new("/runtime/verifier-execution-7")
    );
    assert!(
        pool.identity(HostedProcessScope::VerifierExecution(0))
            .is_err()
    );
}

#[test]
#[cfg(target_os = "linux")]
fn active_run_slots_are_disjoint_from_source_and_each_other() {
    let host = HostedProcessPool::new(10_002, 10_002, 20_000).assert_value();
    let first = host.active_run_slot(0, 65_536).assert_value();
    let second = host.active_run_slot(1, 65_536).assert_value();

    assert_eq!(writer_identity(host), (10_002, 10_002));
    assert_eq!(writer_identity(first), (20_000, 10_002));
    assert_eq!(writer_identity(second), (282_147, 10_002));
    assert_eq!(
        first
            .identity(HostedProcessScope::VerifierExecution(65_536))
            .assert_value()
            .uid(),
        20_000
    );
    assert!(host.active_run_slot(u32::MAX, 65_536).is_err());
    let sentinel = HostedProcessPool::new(1, 1, u32::MAX - 4).assert_value();
    assert!(sentinel.active_run_slot(0, 2).is_err());
}

#[test]
fn new_file_writes_are_exclusive_and_complete() {
    let directory = TestDirectory::new("process-new-file");
    let path = directory.child("value");
    write_new_file(&path, b"complete", 0o600).assert_value();
    assert_eq!(fs::read(&path).assert_value(), b"complete");
    assert!(write_new_file(&path, b"replacement", 0o600).is_err());
    assert_eq!(fs::read(path).assert_value(), b"complete");
}

#[tokio::test]
#[cfg(unix)]
async fn natural_exit_reaps_descendants_without_truncating_slow_stdout() {
    let bytes = PROCESS_STDOUT_CAPACITY * PROCESS_OUTPUT_CHUNK_BYTES + 1;
    let (_cancel, mut process) = stdout_saturating_process(bytes).await;

    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    let mut received = 0;
    while let Some(chunk) = process.recv_stdout().await {
        received += chunk.as_slice().len();
    }
    let completion = tokio::time::timeout(std::time::Duration::from_secs(2), process.wait())
        .await
        .assert_value()
        .assert_value();

    assert_eq!(received, bytes);
    assert_eq!(completion.exit_code, Some(0));
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
    assert_eq!(completion.post_launch_error, None);
}

#[tokio::test]
#[cfg(unix)]
async fn unix_signal_exit_is_distinct_from_an_ordinary_missing_status() {
    let (_cancel, mut process) = shell_process(
        "kill -TERM $$".to_owned(),
        tokio::time::Instant::now() + std::time::Duration::from_secs(2),
    )
    .await;

    while process.recv_stdout().await.is_some() {}
    let completion = wait_for_process(&mut process).await;

    assert_eq!(completion.exit_code, None);
    assert_eq!(completion.termination_signal, Some(libc::SIGTERM));
    assert!(!completion.core_dumped);
    assert!(!completion.timed_out);
}

#[tokio::test]
#[cfg(unix)]
async fn natural_root_exit_reaps_descendants_before_draining_inherited_streams() {
    let (_cancel, mut process) = inherited_stream_process("").await;
    let (stdout, completion) = drain_and_wait(&mut process).await;

    assert_eq!(stdout, b"root-exited\n");
    assert_eq!(completion.stderr_tail, b"root-stderr\n");
    assert_eq!(completion.exit_code, Some(0));
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
    assert!(!completion.timed_out);
    assert_eq!(completion.post_launch_error, None);
}

#[tokio::test]
#[cfg(unix)]
async fn root_crash_reaps_descendants_and_preserves_exit_diagnostics() {
    let (_cancel, mut process) = inherited_stream_process("kill -TERM $$").await;
    let (stdout, completion) = drain_and_wait(&mut process).await;

    assert_eq!(stdout, b"root-exited\n");
    assert_eq!(completion.stderr_tail, b"root-stderr\n");
    assert_eq!(completion.exit_code, None);
    assert_eq!(completion.termination_signal, Some(libc::SIGTERM));
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
    assert!(!completion.timed_out);
    assert_eq!(completion.post_launch_error, None);
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn root_writer_exit_reaps_detached_descendants_without_stopping_peer() {
    // SAFETY: geteuid only inspects the test process identity.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only writer cleanup gate skipped outside the capsule identity");
        return;
    }
    let pool = HostedProcessPool::new(191_002, 191_002, 192_000).assert_value();
    let (cancel, _) = tokio::sync::watch::channel(false);
    let mut peer = writer_process(
        pool,
        2,
        "printf ready; read finish; printf peer-alive",
        &cancel,
    )
    .await;
    assert_eq!(receive_root_output(&mut peer).await, b"ready");
    let script = "/usr/bin/setsid /bin/sh -c 'printf detached-ready; exec /bin/sleep 30' & \
        read finish; printf parent-done";
    let mut process = writer_process(pool, 1, script, &cancel).await;
    assert_eq!(receive_root_output(&mut process).await, b"detached-ready");
    finish_input(&mut process).await;

    let (stdout, completion) = drain_and_wait(&mut process).await;
    assert_eq!(stdout, b"parent-done");
    assert_eq!(completion.exit_code, Some(0));
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
    assert_eq!(completion.post_launch_error, None);

    finish_input(&mut peer).await;
    let (stdout, completion) = drain_and_wait(&mut peer).await;
    assert_eq!(stdout, b"peer-alive");
    assert_eq!(completion.exit_code, Some(0));
    assert!(!completion.cancelled);
    assert_eq!(completion.cleanup, ProcessCleanupEvidence::Reaped);
}

#[tokio::test]
#[cfg(unix)]
async fn release_interrupts_a_saturated_stdout_drain() {
    let bytes = PROCESS_STDOUT_CAPACITY * PROCESS_OUTPUT_CHUNK_BYTES + 1;
    let (_cancel, mut process) = stdout_saturating_process(bytes).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    tokio::time::timeout(std::time::Duration::from_secs(2), process.release())
        .await
        .assert_value()
        .assert_value();
}

#[tokio::test]
#[cfg(unix)]
async fn cancellation_interrupts_a_saturated_stdout_drain() {
    let bytes = PROCESS_STDOUT_CAPACITY * PROCESS_OUTPUT_CHUNK_BYTES + 1;
    let (cancel, mut process) = stdout_saturating_process(bytes).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    cancel.send_replace(true);

    let completion = wait_for_process(&mut process).await;
    assert!(completion.cancelled);
}

#[test]
#[cfg(target_os = "linux")]
fn private_home_create_failure_retains_enoent_without_exposing_the_path() {
    let directory = TestDirectory::new("private-home-cause");
    let missing_root = directory.child("missing-parent/runtime-root");
    let error =
        prepare_local_private_home(&missing_root, HostedProcessScope::Writer).assert_error();
    let detail = launch_detail(error).assert_value();

    assert_os_detail(&detail, "provider private home create failed", libc::ENOENT);
    assert!(!detail.contains(&missing_root.to_string_lossy().into_owned()));
}

#[test]
#[cfg(target_os = "linux")]
fn private_home_followup_failures_retain_their_operation_and_os_cause() {
    let directory = TestDirectory::new("private-home-followup-causes");
    let missing = directory.child("missing-sensitive-home");
    let failures = [
        (
            "provider private home inspection failed",
            super::validate_private_directory(&missing),
        ),
        (
            "provider private home chmod failed",
            super::set_private_directory_mode(&missing),
        ),
        (
            "provider private home chown failed",
            super::set_private_directory_owner(&missing, Some((10_002, 10_002))),
        ),
    ];

    for (operation, failure) in failures {
        let detail = launch_detail(failure.assert_error()).assert_value();
        assert_os_detail(&detail, operation, libc::ENOENT);
        assert!(!detail.contains(&missing.to_string_lossy().into_owned()));
    }
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn spawn_failures_distinguish_enoent_eacces_and_e2big_without_command_values() {
    let missing_program = "/definitely/missing/zeroshot-sensitive-program";
    let missing = spawn_failure(missing_program, Vec::new()).await;
    assert_os_detail(&missing, "process spawn failed", libc::ENOENT);
    assert!(!missing.contains(missing_program));

    let directory = TestDirectory::new("process-spawn-causes");
    let denied_program = directory.child("not-executable-sensitive");
    write_new_file(&denied_program, b"#!/bin/sh\nexit 0\n", 0o600).assert_value();
    let denied = spawn_failure(&denied_program.to_string_lossy(), Vec::new()).await;
    assert_os_detail(&denied, "process spawn failed", libc::EACCES);
    assert!(!denied.contains(&denied_program.to_string_lossy().into_owned()));

    let oversized_marker = "SENSITIVE_ARG_VALUE";
    let oversized = format!("{oversized_marker}{}", "x".repeat(256 * 1024));
    let too_big = spawn_failure("/bin/true", vec![oversized]).await;
    assert_os_detail(&too_big, "process spawn failed", libc::E2BIG);
    assert!(!too_big.contains(oversized_marker));
}

#[cfg(target_os = "linux")]
async fn spawn_failure(program: &str, argv: Vec<String>) -> String {
    let (cancel, cancellation) = tokio::sync::watch::channel(false);
    let result = LocalProcessRunner::new()
        .open(
            process_command(program, argv),
            DriverCancellation::new(cancellation),
        )
        .await;
    drop(cancel);
    launch_detail(result.assert_error()).assert_value()
}

#[cfg(target_os = "linux")]
fn process_command(program: &str, argv: Vec<String>) -> ProcessSessionCommand {
    process_command_with_deadline(
        program,
        argv,
        tokio::time::Instant::now() + std::time::Duration::from_secs(2),
    )
}

#[cfg(target_os = "linux")]
fn launch_detail(error: ProcessRunnerError) -> Option<String> {
    match error {
        ProcessRunnerError::Launch(detail) => Some(detail),
        ProcessRunnerError::InvalidCommand(_) | ProcessRunnerError::Io(_) => None,
    }
}

#[cfg(target_os = "linux")]
fn assert_os_detail(detail: &str, operation: &str, raw_os_error: i32) {
    assert!(detail.starts_with(operation));
    assert!(detail.contains("kind="));
    assert!(detail.contains(&format!("raw_os_error={raw_os_error}")));
    assert!(detail.contains("message="));
}

#[cfg(unix)]
async fn stdout_saturating_process(
    bytes: usize,
) -> (tokio::sync::watch::Sender<bool>, ProcessSession) {
    shell_process(
        format!("sleep 30 & /usr/bin/head -c {bytes} /dev/zero"),
        tokio::time::Instant::now() + std::time::Duration::from_secs(15),
    )
    .await
}

#[cfg(unix)]
async fn shell_process(
    script: String,
    deadline: tokio::time::Instant,
) -> (tokio::sync::watch::Sender<bool>, ProcessSession) {
    let (cancel, cancellation) = tokio::sync::watch::channel(false);
    let process = LocalProcessRunner::new()
        .open(
            process_command_with_deadline("/bin/sh", vec!["-c".to_owned(), script], deadline),
            DriverCancellation::new(cancellation),
        )
        .await
        .assert_value();
    (cancel, process)
}

#[cfg(unix)]
async fn inherited_stream_process(
    ending: &str,
) -> (tokio::sync::watch::Sender<bool>, ProcessSession) {
    shell_process(
        format!(
            "trap '' HUP; sleep 30 & printf 'root-exited\\n'; printf 'root-stderr\\n' >&2; {ending}"
        ),
        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
    )
    .await
}

#[cfg(unix)]
async fn drain_and_wait(process: &mut ProcessSession) -> (Vec<u8>, ProcessSessionOutput) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut stdout = Vec::new();
        while let Some(chunk) = process.recv_stdout().await {
            stdout.extend_from_slice(chunk.as_slice());
        }
        (stdout, process.wait().await.assert_value())
    })
    .await
    .assert_value()
}

#[cfg(target_os = "linux")]
async fn writer_process(
    pool: HostedProcessPool,
    execution: u64,
    script: &str,
    cancellation: &tokio::sync::watch::Sender<bool>,
) -> ProcessSession {
    let identity = pool
        .identity(HostedProcessScope::WriterExecution(execution))
        .assert_value();
    let mut command = process_command_with_deadline(
        "/bin/sh",
        vec!["-c".to_owned(), script.to_owned()],
        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
    );
    command.workspace.current_dir = std::env::temp_dir();
    identity
        .runner()
        .open(command, DriverCancellation::new(cancellation.subscribe()))
        .await
        .assert_value()
}

#[cfg(target_os = "linux")]
async fn finish_input(process: &mut ProcessSession) {
    process
        .send(super::ProcessFrame::new(b"finish\n".to_vec()).assert_value())
        .await
        .assert_value();
}

#[cfg(target_os = "linux")]
async fn receive_root_output(process: &mut ProcessSession) -> Vec<u8> {
    tokio::time::timeout(std::time::Duration::from_secs(2), process.recv_stdout())
        .await
        .assert_value()
        .assert_value()
        .into_inner()
}

#[cfg(unix)]
async fn wait_for_process(process: &mut ProcessSession) -> ProcessSessionOutput {
    tokio::time::timeout(std::time::Duration::from_secs(3), process.wait())
        .await
        .assert_value()
        .assert_value()
}

#[cfg(unix)]
fn process_command_with_deadline(
    program: &str,
    argv: Vec<String>,
    deadline: tokio::time::Instant,
) -> ProcessSessionCommand {
    ProcessSessionCommand {
        program: program.to_owned(),
        argv,
        environment: std::collections::BTreeMap::new(),
        workspace: WorkspaceCapability {
            current_dir: std::env::current_dir().assert_value(),
            mode: WorkspaceAccessMode::ReadOnly,
        },
        deadline: Some(deadline),
    }
}

#[cfg(target_os = "linux")]
fn writer_identity(pool: HostedProcessPool) -> (u32, u32) {
    let identity = pool.identity(HostedProcessScope::Writer).assert_value();
    (identity.uid(), identity.gid())
}

#[test]
#[cfg(target_os = "linux")]
fn writer_and_verifier_memberships_are_disjoint_across_runs() {
    use super::platform::WorkerMembership;

    let host = HostedProcessPool::new(10_002, 10_002, 20_000).assert_value();
    let mut memberships = std::collections::BTreeSet::new();
    for slot in 0..2 {
        let pool = host.active_run_slot(slot, 8).assert_value();
        let owner = pool.identity(HostedProcessScope::Writer).assert_value();
        assert!(memberships.insert(owner.uid()));
        for index in [1, 8] {
            for scope in [
                HostedProcessScope::WriterNodeInstance(index),
                HostedProcessScope::WriterExecution(index),
                HostedProcessScope::VerifierNodeInstance(index),
                HostedProcessScope::VerifierExecution(index),
            ] {
                let identity = pool.identity(scope).assert_value();
                let membership = identity.runner().containment.membership().assert_value();
                assert_eq!(
                    pool.identity(scope)
                        .assert_value()
                        .runner()
                        .containment
                        .membership(),
                    Some(membership)
                );
                let member = match membership {
                    WorkerMembership::SupplementaryGroup(group) => {
                        assert_eq!((identity.uid(), identity.gid()), (owner.uid(), owner.gid()));
                        group
                    }
                    WorkerMembership::Uid(uid) => {
                        assert_ne!(uid, owner.uid());
                        assert_eq!(identity.gid(), 20_000);
                        uid
                    }
                };
                assert!(memberships.insert(member));
            }
        }
        assert!(
            pool.identity(HostedProcessScope::WriterExecution(0))
                .is_err()
        );
        assert!(
            pool.identity(HostedProcessScope::WriterNodeInstance(u64::MAX))
                .is_err()
        );
    }
    for group in [0, u32::MAX] {
        assert!(LocalProcessRunner::hosted_identity(10_002, 10_002, Some(group)).is_err());
    }
}

#[test]
#[cfg(target_os = "linux")]
fn coverage_contract_hosted_identity_boundaries_fail_closed_without_launching_processes() {
    assert_scope_contracts();
    assert_pool_rejection_contracts();
    assert_runner_contracts();
}

#[cfg(target_os = "linux")]
fn assert_scope_contracts() {
    let root = Path::new("/runtime");
    for (scope, leaf, session) in [
        (HostedProcessScope::Writer, "writer", None),
        (
            HostedProcessScope::WriterNodeInstance(7),
            "writer-node-instance-7",
            Some((7, 2)),
        ),
        (
            HostedProcessScope::WriterExecution(8),
            "writer-execution-8",
            Some((8, 3)),
        ),
        (
            HostedProcessScope::VerifierNodeInstance(9),
            "verifier-node-instance-9",
            Some((9, 0)),
        ),
        (
            HostedProcessScope::VerifierExecution(10),
            "verifier-execution-10",
            Some((10, 1)),
        ),
    ] {
        assert_eq!(scope.private_home(root), root.join(leaf));
        assert_eq!(scope.session_identity(), session);
        scope.validate().assert_value();
    }
    for scope in [
        HostedProcessScope::WriterNodeInstance(0),
        HostedProcessScope::WriterExecution(0),
        HostedProcessScope::VerifierNodeInstance(0),
        HostedProcessScope::VerifierExecution(0),
    ] {
        assert!(matches!(
            scope.validate(),
            Err(ProcessRunnerError::InvalidCommand(_))
        ));
    }
}

#[cfg(target_os = "linux")]
fn assert_pool_rejection_contracts() {
    for arguments in [(0, 2, 3), (1, 0, 3), (1, 2, 0), (1, 2, u32::MAX), (3, 2, 3)] {
        assert!(matches!(
            HostedProcessPool::new(arguments.0, arguments.1, arguments.2),
            Err(ProcessRunnerError::InvalidCommand(_))
        ));
    }

    let pool = HostedProcessPool::new(10_002, 10_002, 20_000).assert_value();
    assert_eq!(
        pool.writer()
            .assert_value()
            .containment
            .membership()
            .assert_value(),
        super::platform::WorkerMembership::Uid(10_002)
    );
    assert_eq!(
        pool.verifier(1)
            .assert_value()
            .containment
            .membership()
            .assert_value(),
        super::platform::WorkerMembership::SupplementaryGroup(20_003)
    );
    assert!(pool.active_run_slot(0, u64::MAX).is_err());
    assert!(pool.active_run_slot(u32::MAX, 1).is_err());
    let sentinel = HostedProcessPool::new(1, 1, u32::MAX - 4).assert_value();
    assert!(sentinel.active_run_slot(0, 1).is_err());
    let near_end = HostedProcessPool::new(1, 1, u32::MAX - 1).assert_value();
    assert!(
        near_end
            .identity(HostedProcessScope::VerifierExecution(2))
            .is_err()
    );
}

#[cfg(target_os = "linux")]
fn assert_runner_contracts() {
    assert_eq!(
        ProcessRunnerError::InvalidCommand("bad".to_owned()).launch_evidence(),
        super::ProcessLaunchEvidence::DefinitelyNotStarted
    );
    assert_eq!(
        ProcessRunnerError::Launch("failed".to_owned()).launch_evidence(),
        super::ProcessLaunchEvidence::DefinitelyNotStarted
    );
    assert_eq!(
        ProcessRunnerError::Io("uncertain".to_owned()).launch_evidence(),
        super::ProcessLaunchEvidence::MayHaveStarted
    );
    assert_eq!(
        LocalProcessRunner::default().containment,
        LocalProcessRunner::new().containment
    );
    assert_eq!(
        LocalProcessRunner::hosted_worker()
            .assert_value()
            .containment
            .membership(),
        Some(super::platform::WorkerMembership::Uid(
            super::HOSTED_WORKER_UID,
        ))
    );
    assert!(LocalProcessRunner::hosted_worker_identity(0, 1).is_err());
    assert!(LocalProcessRunner::hosted_worker_identity(1, 0).is_err());
}

#[test]
#[cfg(target_os = "linux")]
fn coverage_contract_command_domain_preflight_checks_supervisor_authority_before_launch() {
    let identity = HostedProcessPool::new(10_002, 10_002, 20_000)
        .assert_value()
        .identity(HostedProcessScope::WriterExecution(65_536))
        .assert_value();
    let result = identity.prepare_command_domain();
    // SAFETY: geteuid only observes this test process's effective identity.
    if unsafe { libc::geteuid() } == 0 {
        result.assert_value();
    } else {
        let detail = result.assert_error().to_string();
        assert!(detail.contains("root supervisor"));
    }

    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let invalid = PathBuf::from(OsString::from_vec(b"invalid\0home".to_vec()));
    assert!(matches!(
        super::set_private_directory_owner(&invalid, Some((10_002, 10_002))),
        Err(ProcessRunnerError::InvalidCommand(_))
    ));
}
