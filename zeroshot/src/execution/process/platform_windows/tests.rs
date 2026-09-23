use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;

use tokio::time::{Duration, Instant, timeout};

use super::{INJECT_ASSIGN_FAILURE, INJECT_RESUME_FAILURE, JOB_CLOSE_COUNT, WINDOWS_TEST_LOCK};
use super::super::platform::{
    ProcessCleanupEvidence, ProcessContainment, ProcessTreeRegistration, capture_process_tree,
    configure_process, register_process_tree_for, terminate_process_tree,
};

#[test]
fn an_already_running_thread_does_not_acknowledge_process_resume() {
    let current = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    assert!(!super::resume_thread(current).unwrap());
}

fn spawn_suspended_sentinel(
    name: &str,
) -> (ProcessTreeRegistration, tokio::process::Child, PathBuf) {
    let registration = register_process_tree_for(ProcessContainment::ProcessGroup).unwrap();
    let system_root = std::env::var("SystemRoot").expect("Windows has SystemRoot");
    let program = PathBuf::from(&system_root).join("System32").join("cmd.exe");
    let sentinel = std::env::temp_dir().join(format!("{name}-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&sentinel);
    let script = format!(
        "echo first>\"{}\" & ping -n 31 127.0.0.1 >NUL",
        sentinel.display()
    );
    let mut command = tokio::process::Command::new(program);
    use std::os::windows::process::CommandExt;
    command.args(["/D", "/S", "/C"]);
    command.as_std_mut().raw_arg(format!("\"{script}\""));
    command.current_dir(std::env::temp_dir());
    command.env_clear();
    command.env("SystemRoot", system_root);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    command.kill_on_drop(true);
    configure_process(&mut command, ProcessContainment::ProcessGroup);
    let child = command.spawn().unwrap();
    super::TEST_PROCESS_ID.store(child.id().unwrap(), Ordering::SeqCst);
    (registration, child, sentinel)
}

async fn assert_still_suspended(sentinel: &Path) {
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !sentinel.exists(),
        "first instruction ran before Job assignment"
    );
}

async fn wait_for_sentinel(sentinel: &Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !sentinel.exists() {
        assert!(
            Instant::now() < deadline,
            "resumed process did not run its first instruction"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn reset_injections() {
    INJECT_ASSIGN_FAILURE.store(false, Ordering::SeqCst);
    INJECT_RESUME_FAILURE.store(false, Ordering::SeqCst);
    JOB_CLOSE_COUNT.store(0, Ordering::SeqCst);
}

#[tokio::test]
async fn real_suspended_spawn_cannot_run_before_assignment() {
    let _serial = WINDOWS_TEST_LOCK.lock().await;
    reset_injections();
    let (registration, mut child, sentinel) =
        spawn_suspended_sentinel("zeroshot-windows-contained");
    assert_still_suspended(&sentinel).await;
    let process_tree = capture_process_tree(registration, &mut child).unwrap();
    wait_for_sentinel(&sentinel).await;
    let termination = terminate_process_tree(&process_tree, &mut child).await;
    assert_eq!(termination.cleanup, ProcessCleanupEvidence::Reaped);
    drop(process_tree);
    assert_eq!(JOB_CLOSE_COUNT.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_file(sentinel);
}

#[tokio::test]
async fn assignment_failure_terminates_and_reaps_without_first_instruction() {
    let _serial = WINDOWS_TEST_LOCK.lock().await;
    reset_injections();
    let (registration, mut child, sentinel) =
        spawn_suspended_sentinel("zeroshot-windows-assign-failure");
    assert_still_suspended(&sentinel).await;
    INJECT_ASSIGN_FAILURE.store(true, Ordering::SeqCst);
    assert!(capture_process_tree(registration, &mut child).is_err());
    timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("failed assignment root was not reaped")
        .unwrap();
    assert!(!sentinel.exists());
    assert_eq!(JOB_CLOSE_COUNT.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn resume_failure_terminates_and_reaps_without_first_instruction() {
    let _serial = WINDOWS_TEST_LOCK.lock().await;
    reset_injections();
    let (registration, mut child, sentinel) =
        spawn_suspended_sentinel("zeroshot-windows-resume-failure");
    assert_still_suspended(&sentinel).await;
    INJECT_RESUME_FAILURE.store(true, Ordering::SeqCst);
    assert!(capture_process_tree(registration, &mut child).is_err());
    timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("failed resume root was not reaped")
        .unwrap();
    assert!(!sentinel.exists());
    assert_eq!(JOB_CLOSE_COUNT.load(Ordering::SeqCst), 1);
}
