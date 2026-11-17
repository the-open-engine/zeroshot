use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, FILETIME, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{TOKEN_IMPERSONATE, TOKEN_QUERY};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

use super::ProcessIdentity;

const MAX_SNAPSHOT_PROCESSES: usize = 65_536;
const MAX_NATIVE_PATH_UNITS: usize = 32_768;

pub(crate) fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
    if pid == 4 {
        return system_identity();
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        let error = io::Error::last_os_error();
        return Err(
            if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "native process is no longer present",
                )
            } else {
                error
            },
        );
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    let born = root_born(process.as_raw_handle())?;
    let parent = if is_native_shell(pid) {
        0
    } else {
        process_parent(pid)?
    };
    Ok(ProcessIdentity {
        pid,
        parent,
        group: 0,
        born,
    })
}

/// Reads the held root handle even after exit; borrows it without closing it.
pub(crate) fn root_born(process: RawHandle) -> io::Result<u64> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn system_identity() -> io::Result<ProcessIdentity> {
    // Only the native System PID with a verified kernel parent terminates ancestry.
    if process_parent(4)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native System ancestry is invalid",
        ));
    }
    Ok(ProcessIdentity {
        pid: 4,
        parent: 0,
        group: 0,
        born: 0,
    })
}

fn is_native_shell(pid: u32) -> bool {
    let window = unsafe { GetShellWindow() };
    if window.is_null() {
        return false;
    }
    let mut shell = 0;
    // The OS shell window locates the interactive user boundary without labels or environment.
    unsafe { GetWindowThreadProcessId(window, &mut shell) };
    shell == pid
}

fn process_parent(pid: u32) -> io::Result<u32> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..PROCESSENTRY32W::default()
    };
    if unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } == 0 {
        return Err(snapshot_error());
    }
    for _ in 0..MAX_SNAPSHOT_PROCESSES {
        if entry.th32ProcessID == pid {
            return Ok(entry.th32ParentProcessID);
        }
        if unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } == 0 {
            return Err(snapshot_error());
        }
    }
    Err(io::Error::other(
        "native process snapshot exceeds the inspection bound",
    ))
}

fn snapshot_error() -> io::Error {
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
        io::Error::new(
            io::ErrorKind::NotFound,
            "native process is absent from the snapshot",
        )
    } else {
        error
    }
}

pub(crate) fn registry_directory() -> io::Result<PathBuf> {
    let token = current_user_token()?;
    let mut pointer = std::ptr::null_mut();
    // Resolve the current native user through its process token, not inherited path variables.
    let result = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_LocalAppData,
            0,
            token.as_raw_handle(),
            &mut pointer,
        )
    };
    let allocation = KnownFolderPath(pointer);
    if result < 0 {
        return Err(io::Error::other(format!(
            "native local application data lookup failed ({result:#x})"
        )));
    }
    Ok(known_folder_path(allocation.0)?.join("zeroshot-process-authority"))
}

fn current_user_token() -> io::Result<OwnedHandle> {
    let mut token = std::ptr::null_mut();
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_IMPERSONATE,
            &mut token,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

struct KnownFolderPath(*mut u16);

impl Drop for KnownFolderPath {
    fn drop(&mut self) {
        // The API requires freeing the returned allocation on both success and failure.
        unsafe { CoTaskMemFree(self.0.cast()) };
    }
}

fn known_folder_path(pointer: *const u16) -> io::Result<PathBuf> {
    if pointer.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native local application data path is absent",
        ));
    }
    for length in 0..MAX_NATIVE_PATH_UNITS {
        // SHGetKnownFolderPath supplies a NUL-terminated allocation; bound the scan explicitly.
        if unsafe { *pointer.add(length) } == 0 {
            let units = unsafe { std::slice::from_raw_parts(pointer, length) };
            return Ok(PathBuf::from(OsString::from_wide(units)));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "native local application data path exceeds the inspection bound",
    ))
}
