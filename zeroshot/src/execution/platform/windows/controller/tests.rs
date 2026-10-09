use super::*;

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use std::os::windows::process::CommandExt;

use windows_sys::Win32::Foundation::STATUS_CONTROL_C_EXIT;
use windows_sys::Win32::System::Console::{
    GenerateConsoleCtrlEvent, GetConsoleCP, GetConsoleProcessList, GetConsoleWindow,
};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, GetCurrentProcess, GetExitCodeProcess};
use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;

const MODE: &str = "ZEROSHOT_CONTROLLER_JOB_FIXTURE";
const CONSOLE_MODE: &str = "ZEROSHOT_CONTROLLER_CONSOLE_FIXTURE";
// The controller's stdio is discarded, so console fixtures report through files.
const RESULT: &str = "ZEROSHOT_CONTROLLER_CONSOLE_RESULT";
const OWNER: &str = "ZEROSHOT_CONTROLLER_CONSOLE_OWNER";
const JOB_TEST: &str =
    "execution::platform::windows::controller::tests::controller_detachment_respects_job_hierarchy";
const CONSOLE_TEST: &str =
    "execution::platform::windows::controller::tests::controller_console_is_hidden_and_shared";
const BREAK_TEST: &str =
    "execution::platform::windows::controller::tests::guarded_controller_survives_console_break";
const CALLER_TEST: &str =
    "execution::platform::windows::controller::tests::controller_ignores_caller_console_break";

#[test]
fn controller_console_guard_handles_only_interrupts() {
    use windows_sys::Win32::System::Console::{
        CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };
    for (event, handled) in [
        (CTRL_C_EVENT, true),
        (CTRL_BREAK_EVENT, true),
        (CTRL_CLOSE_EVENT, false),
        (CTRL_LOGOFF_EVENT, false),
        (CTRL_SHUTDOWN_EVENT, false),
    ] {
        assert_eq!(
            unsafe { ignore_console_interrupt(event) } != 0,
            handled,
            "{event}"
        );
    }
    guard_controller_console().unwrap();
}

#[tokio::test]
async fn controller_detachment_respects_job_hierarchy() {
    if let Ok(mode) = std::env::var(MODE) {
        if mode == "detached" {
            tokio::time::sleep(Duration::from_secs(60)).await;
            return;
        }
        exercise_job_hierarchy(&mode).await;
        return;
    }
    // Job membership is irreversible, so each case owns a separate test process.
    for mode in [
        "restricted",
        "nested-explicit",
        "nested-silent",
        "explicit",
        "silent",
    ] {
        let mut command = fixture_command(JOB_TEST, MODE, mode);
        let output = tokio::time::timeout(Duration::from_secs(30), command.output())
            .await
            .expect("Job fixture timed out")
            .expect("Job fixture launched");
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{mode}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

async fn exercise_job_hierarchy(mode: &str) {
    let mut jobs = Vec::new();
    if mode.starts_with("nested-") {
        jobs.push(join_job(0));
    }
    let flags = if mode.ends_with("explicit") {
        JOB_OBJECT_LIMIT_BREAKAWAY_OK
    } else if mode.ends_with("silent") {
        JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK
    } else {
        0
    };
    jobs.push(join_job(flags));
    let mut command = fixture_command(JOB_TEST, MODE, "detached");
    let should_succeed = matches!(mode, "explicit" | "silent");
    match spawn_controller(&mut command) {
        Ok(mut child) => {
            let detached = !in_job(child.0.as_raw_handle()).unwrap();
            child.kill().await.unwrap();
            assert!(
                should_succeed,
                "controller started inside a restrictive Job"
            );
            assert!(detached, "controller retained a caller-owned Job");
        }
        Err(error) => {
            assert!(!should_succeed, "permitted breakaway failed: {error}");
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            if mode == "restricted" {
                assert!(error.to_string().contains("cannot detach a controller"));
            }
        }
    }
    drop(jobs);
}

fn join_job(flags: u32) -> OwnedHandle {
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    assert!(!job.is_null());
    let job = unsafe { OwnedHandle::from_raw_handle(job) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // The fixture process owns these Jobs; omitting kill-on-close lets its test harness exit normally.
    limits.BasicLimitInformation.LimitFlags = flags;
    check(unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    })
    .unwrap();
    check(unsafe { AssignProcessToJobObject(job.as_raw_handle(), GetCurrentProcess()) }).unwrap();
    job
}

fn fixture_command(test: &str, variable: &str, mode: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture"])
        .env_clear()
        .envs(std::env::vars_os())
        .env(variable, mode)
        .kill_on_drop(true);
    command
}

#[tokio::test]
async fn controller_console_is_hidden_and_shared() {
    if let Ok(mode) = std::env::var(CONSOLE_MODE) {
        match mode.as_str() {
            "controller" => report(inspect_controller_console()),
            "grandchild" => inspect_grandchild_console(),
            _ => panic!("unknown console fixture {mode}"),
        }
        return;
    }
    let result = Scratch::new("console");
    let mut command = fixture_command(CONSOLE_TEST, CONSOLE_MODE, "controller");
    command.env(RESULT, &result.0);
    let mut child = spawn_controller(&mut command).unwrap();
    let exited = wait_for(Duration::from_secs(60), || {
        child.try_wait().unwrap().is_some()
    })
    .await;
    if !exited {
        child.kill().await.unwrap();
    }
    assert_eq!(
        result.read().as_deref(),
        Some("ok"),
        "controller exited: {exited}"
    );
}

#[tokio::test]
async fn guarded_controller_survives_console_break() {
    if let Ok(mode) = std::env::var(CONSOLE_MODE) {
        match mode.as_str() {
            "guarded" => {
                guard_controller_console().unwrap();
                break_own_console();
            }
            "unguarded" => break_own_console(),
            _ => panic!("unknown Ctrl-Break fixture {mode}"),
        }
        return;
    }
    let marker = Scratch::new("guarded");
    let mut command = fixture_command(BREAK_TEST, CONSOLE_MODE, "guarded");
    command.env(RESULT, &marker.0);
    let mut child = spawn_controller(&mut command).unwrap();
    let reported = wait_for(Duration::from_secs(30), || {
        marker.read().is_some() || child.try_wait().unwrap().is_some()
    })
    .await;
    let running = child.try_wait().unwrap().is_none();
    let code = exit_code(&child);
    child.kill().await.unwrap();
    assert!(
        reported && running,
        "guarded controller exited with {code:#x}"
    );
    assert_eq!(marker.read().as_deref(), Some("survived"));

    // Without the guard the same event must terminate the controller, proving it was delivered.
    let marker = Scratch::new("unguarded");
    let mut command = fixture_command(BREAK_TEST, CONSOLE_MODE, "unguarded");
    command.env(RESULT, &marker.0);
    let mut child = spawn_controller(&mut command).unwrap();
    let exited = wait_for(Duration::from_secs(30), || {
        child.try_wait().unwrap().is_some()
    })
    .await;
    if !exited {
        child.kill().await.unwrap();
    }
    assert!(exited, "unguarded controller ignored Ctrl-Break");
    assert_eq!(exit_code(&child), STATUS_CONTROL_C_EXIT as u32);
    assert_eq!(marker.read(), None);
}

#[tokio::test]
async fn controller_ignores_caller_console_break() {
    if let Ok(mode) = std::env::var(CONSOLE_MODE) {
        match mode.as_str() {
            "caller" => break_caller_console().await,
            "idle" => tokio::time::sleep(Duration::from_secs(60)).await,
            _ => panic!("unknown caller fixture {mode}"),
        }
        return;
    }
    // The caller owns a separate hidden console, so its Ctrl-Break cannot reach this harness.
    let mut command = fixture_command(CALLER_TEST, CONSOLE_MODE, "caller");
    command.creation_flags(CREATE_NO_WINDOW);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("caller fixture timed out")
        .expect("caller fixture launched");
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn inspect_controller_console() -> Result<(), String> {
    if unsafe { GetConsoleCP() } == 0 {
        return Err("controller has no console".into());
    }
    if console_window_visible() {
        return Err("controller console window is visible".into());
    }
    // The controller breaks away from the harness Job, so its own kill-on-close Job stops cmd.exe
    // and the grandchild whenever it exits, including when the harness kills it after a timeout.
    // Only process exit closes the handle; closing it earlier would also kill the controller.
    std::mem::forget(join_job(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE));
    // Launch like production, without console flags, and through cmd.exe as npm shims are.
    let mut line = std::ffi::OsString::from("/d /s /c \"\"");
    line.push(std::env::current_exe().unwrap());
    line.push(format!("\" --exact {CONSOLE_TEST} --nocapture\""));
    let grandchild = std::process::Command::new("cmd.exe")
        .raw_arg(line)
        .env(CONSOLE_MODE, "grandchild")
        .env(OWNER, std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("grandchild launch failed: {error}"))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || sender.send(grandchild.wait_with_output()));
    // Report before the harness's 60-second limit so a hang surfaces as this failure.
    let output = receiver
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| "grandchild timed out".to_owned())?
        .map_err(|error| format!("grandchild wait failed: {error}"))?;
    if output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed") {
        return Ok(());
    }
    Err(format!(
        "grandchild failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

fn inspect_grandchild_console() {
    let owner: u32 = std::env::var(OWNER).unwrap().parse().unwrap();
    let mut processes = [0_u32; 64];
    let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) };
    let attached = &processes[..(count as usize).min(processes.len())];
    assert!(count >= 2, "console has {count} processes");
    assert!(
        attached.contains(&owner),
        "controller {owner} is not in {attached:?}"
    );
    assert!(
        !console_window_visible(),
        "grandchild console window is visible"
    );
}

fn console_window_visible() -> bool {
    let window = unsafe { GetConsoleWindow() };
    !window.is_null() && unsafe { IsWindowVisible(window) } != 0
}

// Console handlers run on an injected thread, so give delivery time before reporting survival.
fn break_own_console() {
    check(unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, 0) }).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    std::fs::write(std::env::var_os(RESULT).unwrap(), "survived").unwrap();
    std::thread::sleep(Duration::from_secs(60));
}

async fn break_caller_console() {
    guard_controller_console().unwrap();
    let mut command = fixture_command(CALLER_TEST, CONSOLE_MODE, "idle");
    let mut controller = spawn_controller(&mut command).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    check(unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, 0) }).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let running = controller.try_wait().unwrap().is_none();
    let code = exit_code(&controller);
    controller.kill().await.unwrap();
    assert!(running, "unguarded controller exited with {code:#x}");
}

fn report(verdict: Result<(), String>) {
    let text = verdict.err().unwrap_or_else(|| "ok".into());
    std::fs::write(std::env::var_os(RESULT).unwrap(), text).unwrap();
}

fn exit_code(child: &ControllerChild) -> u32 {
    let mut code = 0;
    check(unsafe { GetExitCodeProcess(child.0.as_raw_handle(), &mut code) }).unwrap();
    code
}

async fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    while !done() {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    true
}

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "zeroshot-controller-{label}-{}-{nanos}",
            std::process::id()
        )))
    }

    fn read(&self) -> Option<String> {
        std::fs::read_to_string(&self.0).ok()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
