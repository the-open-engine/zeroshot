//! Detached controller creation with no inherited handles, including extra pipeline handles
//! that PowerShell may pass in addition to the child's standard input/output/error.
use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
    TerminateProcess, WaitForSingleObject,
};

use super::{check, detached_creation_flags};

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
            detached_creation_flags()? | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr().cast(),
            directory
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            &startup,
            &mut process,
        )
    })?;
    unsafe {
        CloseHandle(process.hThread);
    }
    Ok(ControllerChild(unsafe {
        OwnedHandle::from_raw_handle(process.hProcess)
    }))
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
