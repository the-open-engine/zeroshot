//! Detached controller creation on a new windowless console with no inherited handles, including
//! extra pipeline handles that PowerShell may pass in addition to the child's standard streams.
use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION,
    ResumeThread, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};

use super::check;

#[cfg(test)]
mod tests;

pub(crate) struct ControllerChild(OwnedHandle);

impl ControllerChild {
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<()>> {
        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => Ok(Some(())),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    pub(crate) async fn kill(&mut self) -> io::Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        check(unsafe { TerminateProcess(self.0.as_raw_handle(), 1) })?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.try_wait()?.is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(())
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "controller exit timed out"))?
    }
}

/// Keeps console interrupts from terminating the controller. `SetConsoleCtrlHandler(None, TRUE)`
/// ignores only Ctrl-C, so Ctrl-Break needs a handler routine; registration is not inherited by
/// children. Close, logoff, and shutdown events keep their default handling.
pub(crate) fn guard_controller_console() -> io::Result<()> {
    check(unsafe { SetConsoleCtrlHandler(Some(ignore_console_interrupt), 1) })
}

unsafe extern "system" fn ignore_console_interrupt(event: u32) -> BOOL {
    BOOL::from(matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT))
}

/// The caller supplies the complete environment (its Command uses env_clear). This private
/// launch path intentionally discards stdio and handles instead of inheriting the caller's pipes.
pub(crate) fn spawn_controller(
    command: &mut tokio::process::Command,
) -> io::Result<ControllerChild> {
    let command = command.as_std();
    let program = wide(command.get_program())?;
    let mut arguments = Vec::new();
    for value in std::iter::once(command.get_program()).chain(command.get_args()) {
        if !arguments.is_empty() {
            arguments.push(u16::from(b' '));
        }
        quote_argument(&mut arguments, value)?;
    }
    arguments.push(0);
    let environment = environment_block(command)?;
    let directory = command
        .get_current_dir()
        .map(|path| wide(path.as_os_str()))
        .transpose()?;
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    check(unsafe {
        CreateProcessW(
            program.as_ptr(),
            arguments.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            detached_creation_flags()? | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
            environment.as_ptr().cast(),
            directory
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            &startup,
            &mut process,
        )
    })?;
    resume_controller(process)
}

fn resume_controller(process: PROCESS_INFORMATION) -> io::Result<ControllerChild> {
    let child = ControllerChild(unsafe { OwnedHandle::from_raw_handle(process.hProcess) });
    let thread = unsafe { OwnedHandle::from_raw_handle(process.hThread) };
    if let Err(error) = resume_detached(&child, &thread) {
        check(unsafe { TerminateProcess(child.0.as_raw_handle(), 1) })?;
        if unsafe { WaitForSingleObject(child.0.as_raw_handle(), 5_000) } != WAIT_OBJECT_0 {
            return Err(io::Error::other(
                "failed controller launch cleanup could not be confirmed",
            ));
        }
        return Err(error);
    }
    Ok(child)
}

fn resume_detached(child: &ControllerChild, thread: &OwnedHandle) -> io::Result<()> {
    // An ancestor Job may prohibit breakaway even when the immediate Job permits it.
    // Check the suspended child before it can consume the bootstrap or start any work.
    if in_job(child.0.as_raw_handle())? {
        return Err(detachment_denied());
    }
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn in_job(process: HANDLE) -> io::Result<bool> {
    let mut present = 0;
    check(unsafe { IsProcessInJob(process, std::ptr::null_mut(), &mut present) })?;
    Ok(present != 0)
}

fn detachment_denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "cannot detach a controller from this Windows Job; run Zeroshot from a terminal that permits process breakaway",
    )
}

// A parent without a console makes every console child open its own visible console, so the
// controller gets a new windowless console that its console descendants share. Being a new console
// keeps the invoking terminal's Ctrl-C and close events out; controller mode guards against
// Ctrl-Break sent to the hidden console. Leave caller-owned Jobs only through their permitted
// breakaway policy.
fn detached_creation_flags() -> io::Result<u32> {
    use windows_sys::Win32::System::JobObjects::{
        QueryInformationJobObject, JobObjectExtendedLimitInformation,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
    };
    // Windows ignores CREATE_NO_WINDOW when it is combined with a detached or new-console flag.
    let mut flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
    if in_job(unsafe { GetCurrentProcess() })? {
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        check(unsafe {
            QueryInformationJobObject(
                std::ptr::null_mut(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_mut(&mut limits).cast(),
                std::mem::size_of_val(&limits) as u32,
                std::ptr::null_mut(),
            )
        })?;
        if limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_BREAKAWAY_OK != 0 {
            flags |= CREATE_BREAKAWAY_FROM_JOB;
        } else if limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK
            == 0
        {
            return Err(detachment_denied());
        }
    }
    Ok(flags)
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut encoded: Vec<_> = value.encode_wide().collect();
    if encoded.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process value contains NUL",
        ));
    }
    encoded.push(0);
    Ok(encoded)
}

fn environment_block(command: &std::process::Command) -> io::Result<Vec<u16>> {
    let mut block = Vec::new();
    for (name, value) in command
        .get_envs()
        .filter_map(|(name, value)| value.map(|value| (name, value)))
    {
        let mut entry = name.to_os_string();
        entry.push("=");
        entry.push(value);
        block.extend(wide(&entry)?);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

// Win32 argv quoting, without a command interpreter. Backslashes are doubled only before
// a quote (including the closing quote); embedded quotes get one additional escaping slash.
fn quote_argument(output: &mut Vec<u16>, value: &OsStr) -> io::Result<()> {
    output.push(u16::from(b'"'));
    let mut slashes = 0;
    for unit in wide(value)?.into_iter().take_while(|unit| *unit != 0) {
        if unit == u16::from(b'\\') {
            slashes += 1;
            continue;
        }
        let escaping = if unit == u16::from(b'"') {
            slashes + 1
        } else {
            0
        };
        output.extend(std::iter::repeat_n(u16::from(b'\\'), slashes + escaping));
        output.push(unit);
        slashes = 0;
    }
    output.extend(std::iter::repeat_n(u16::from(b'\\'), slashes * 2));
    output.push(u16::from(b'"'));
    Ok(())
}
