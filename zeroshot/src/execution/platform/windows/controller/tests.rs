use super::*;

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

const MODE: &str = "ZEROSHOT_CONTROLLER_JOB_FIXTURE";

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
        let mut command = fixture_command(mode);
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
    let mut command = fixture_command("detached");
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

fn fixture_command(mode: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "execution::platform::windows::controller::tests::controller_detachment_respects_job_hierarchy",
            "--nocapture",
        ])
        .env_clear()
        .envs(std::env::vars_os())
        .env(MODE, mode)
        .kill_on_drop(true);
    command
}
