use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use tokio::process::Command;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

trait WindowsContainmentCalls {
    fn assign(&mut self) -> Result<(), io::Error>;
    fn resume(&mut self) -> Result<(), io::Error>;
    fn terminate_failed_capture(&mut self);
}

fn assign_suspended_process(calls: &mut impl WindowsContainmentCalls) -> Result<(), io::Error> {
    if let Err(error) = calls.assign() {
        calls.terminate_failed_capture();
        return Err(error);
    }
    if let Err(error) = calls.resume() {
        calls.terminate_failed_capture();
        return Err(error);
    }
    Ok(())
}

struct SystemWindowsContainmentCalls<'a> {
    job: usize,
    process: HANDLE,
    process_id: u32,
    child: &'a mut tokio::process::Child,
}

#[cfg(test)]
static WINDOWS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[cfg(test)]
static INJECT_ASSIGN_FAILURE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
static INJECT_RESUME_FAILURE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
static TEST_PROCESS_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
#[cfg(test)]
static TEST_JOB: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static JOB_CLOSE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl WindowsContainmentCalls for SystemWindowsContainmentCalls<'_> {
    fn assign(&mut self) -> Result<(), io::Error> {
        #[cfg(test)]
        if self.process_id == TEST_PROCESS_ID.load(std::sync::atomic::Ordering::SeqCst) {
            TEST_JOB.store(self.job, std::sync::atomic::Ordering::SeqCst);
        }
        #[cfg(test)]
        if self.injected_error(&INJECT_ASSIGN_FAILURE) {
            return Err(io::Error::other("injected assignment failure"));
        }
        let assigned = unsafe { AssignProcessToJobObject(self.job as HANDLE, self.process) };
        if assigned == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn resume(&mut self) -> Result<(), io::Error> {
        #[cfg(test)]
        if self.injected_error(&INJECT_RESUME_FAILURE) {
            return Err(io::Error::other("injected resume failure"));
        }
        resume_suspended_process(self.process_id)
    }

    fn terminate_failed_capture(&mut self) {
        unsafe {
            TerminateJobObject(self.job as HANDLE, 1);
        }
        let _ = self.child.start_kill();
    }
}

pub(super) fn assign_process_tree(
    job: usize,
    child: &mut tokio::process::Child,
) -> Result<(), io::Error> {
    let process = child
        .raw_handle()
        .ok_or_else(|| io::Error::other("spawned process handle is unavailable"))?;
    let process_id = child
        .id()
        .ok_or_else(|| io::Error::other("spawned process id is unavailable"))?;
    let mut calls = SystemWindowsContainmentCalls {
        job,
        process: process as HANDLE,
        process_id,
        child,
    };
    assign_suspended_process(&mut calls)
}

pub(super) fn configure_process_group(command: &mut Command) {
    command.creation_flags(CREATE_SUSPENDED);
}

pub(super) fn kill_process_tree(job: usize, child: &mut tokio::process::Child) -> Vec<String> {
    let terminated = unsafe { TerminateJobObject(job as HANDLE, 1) };
    if terminated != 0 {
        return Vec::new();
    }
    let termination_error = io::Error::last_os_error();
    let mut errors = vec![super::io_error_detail(
        "Windows job termination failed",
        &termination_error,
    )];
    if let Err(error) = child.start_kill() {
        errors.push(super::io_error_detail(
            "root process termination fallback failed",
            &error,
        ));
    }
    errors
}

fn resume_suspended_process(process_id: u32) -> Result<(), io::Error> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(std::mem::size_of::<THREADENTRY32>())
            .expect("thread entry fits in u32"),
        ..THREADENTRY32::default()
    };
    let mut has_entry = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
    while has_entry {
        if entry.th32OwnerProcessID == process_id && resume_thread(entry.th32ThreadID)? {
            return Ok(());
        }
        has_entry = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Err(io::Error::other(
        "spawned process suspended thread is unavailable",
    ))
}

fn resume_thread(thread_id: u32) -> io::Result<bool> {
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if thread.is_null() {
        return Err(io::Error::last_os_error());
    }
    let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
    match unsafe { ResumeThread(thread.as_raw_handle()) } {
        u32::MAX => Err(io::Error::last_os_error()),
        // Running auxiliary threads do not prove that the suspended launch thread resumed.
        0 => Ok(false),
        _ => Ok(true),
    }
}

pub(super) fn create_kill_on_close_job() -> Result<usize, io::Error> {
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            u32::try_from(std::mem::size_of_val(&limits)).expect("job limits fit in u32"),
        )
    };
    if configured == 0 {
        let error = io::Error::last_os_error();
        unsafe {
            CloseHandle(job);
        }
        return Err(error);
    }
    Ok(job as usize)
}

pub(super) fn job_has_live_members(job: usize) -> Result<bool, io::Error> {
    let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    let queried = unsafe {
        QueryInformationJobObject(
            job as HANDLE,
            JobObjectBasicAccountingInformation,
            std::ptr::from_mut(&mut accounting).cast(),
            u32::try_from(std::mem::size_of_val(&accounting)).expect("job accounting fits in u32"),
            std::ptr::null_mut(),
        )
    };
    if queried == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(accounting.ActiveProcesses != 0)
    }
}

pub(super) fn close_job(job: usize) {
    #[cfg(test)]
    if TEST_JOB
        .compare_exchange(
            job,
            0,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_ok()
    {
        JOB_CLOSE_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    unsafe {
        CloseHandle(job as HANDLE);
    }
}

#[cfg(test)]
impl SystemWindowsContainmentCalls<'_> {
    fn injected_error(&self, flag: &std::sync::atomic::AtomicBool) -> bool {
        use std::sync::atomic::Ordering;
        self.process_id == TEST_PROCESS_ID.load(Ordering::SeqCst)
            && flag.swap(false, Ordering::SeqCst)
    }
}

#[cfg(test)]
#[path = "platform_windows/tests.rs"]
mod tests;
