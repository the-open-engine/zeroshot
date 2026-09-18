use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileInformationByHandle, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    OPEN_ALWAYS, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
};

use super::FileAccess;
mod controller;
pub(crate) mod security;
pub(crate) use controller::{ControllerChild, spawn_controller};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileIdentity {
    volume: u32,
    index: u64,
}

pub(crate) fn file_identity(file: &File) -> io::Result<FileIdentity> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    check(unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) })?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("reparse points are not permitted"));
    }
    Ok(FileIdentity {
        volume: info.dwVolumeSerialNumber,
        index: u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow),
    })
}

pub(crate) fn open_identity(path: &Path) -> io::Result<File> {
    open(
        path,
        FILE_READ_ATTRIBUTES | READ_CONTROL,
        OPEN_EXISTING,
        None,
    )
}

pub(crate) fn private_directory(path: &Path) -> io::Result<()> {
    create_directory(path)?;
    let file = open(
        path,
        FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC,
        OPEN_EXISTING,
        None,
    )?;
    if !file.metadata()?.is_dir() {
        return Err(io::Error::other("private path is not a directory"));
    }
    security::protect(file.as_raw_handle())
}

pub(crate) fn private_file(path: &Path, access: FileAccess) -> io::Result<File> {
    let descriptor = security::PrivateSecurity::new()?;
    let (rights, disposition) = file_access(access);
    let file = open(path, rights, disposition, Some(&descriptor))?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("private path is not a file"));
    }
    if !matches!(access, FileAccess::Read) {
        security::protect(file.as_raw_handle())?;
    }
    validate_private_file(&file)?;
    Ok(file)
}

pub(crate) fn validate_private_file(file: &File) -> io::Result<()> {
    security::validate(file.as_raw_handle())
}

fn open(
    path: &Path,
    rights: u32,
    disposition: u32,
    security: Option<&security::PrivateSecurity>,
) -> io::Result<File> {
    let attributes = security.map(security::PrivateSecurity::attributes);
    let handle = unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            rights,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            attributes
                .as_ref()
                .map_or(std::ptr::null(), std::ptr::from_ref),
            disposition,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    file_identity(&file)?;
    Ok(file)
}

pub(crate) fn commit_file(temporary: &Path, destination: &Path, _parent: &Path) -> io::Result<()> {
    check(unsafe {
        MoveFileExW(
            wide(temporary)?.as_ptr(),
            wide(destination)?.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    })
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains NUL",
        ));
    }
    value.push(0);
    Ok(value)
}

pub(super) fn check(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn create_directory(path: &Path) -> io::Result<()> {
    if !path.exists() {
        if let Some(parent) = path.parent().filter(|parent| !parent.exists()) {
            private_directory(parent)?;
        }
        if let Err(error) = create_private_directory(path) {
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
    }
    Ok(())
}

fn file_access(access: FileAccess) -> (u32, u32) {
    match access {
        FileAccess::Read => (GENERIC_READ, OPEN_EXISTING),
        FileAccess::ReadWrite => (GENERIC_READ | GENERIC_WRITE | WRITE_DAC, OPEN_ALWAYS),
        FileAccess::CreateNew => (GENERIC_READ | GENERIC_WRITE | WRITE_DAC, CREATE_NEW),
    }
}

/// Creates exactly one new private directory, preserving collision detection.
pub(crate) fn create_private_directory(path: &Path) -> io::Result<()> {
    let descriptor = security::PrivateSecurity::new()?;
    let attributes = descriptor.attributes();
    check(unsafe { CreateDirectoryW(wide(path)?.as_ptr(), &attributes) })
}

/// A remote shell may own a kill-on-close Job. Leave it only when that Job permits breakaway;
/// restrictive caller-owned Jobs retain their containment policy.
pub(crate) fn detached_creation_flags() -> io::Result<u32> {
    use windows_sys::Win32::System::JobObjects::{
        IsProcessInJob, QueryInformationJobObject, JobObjectExtendedLimitInformation,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
    };
    let mut flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    let mut in_job = 0;
    check(unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) })?;
    if in_job != 0 {
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
        }
    }
    Ok(flags)
}

pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    let file = open_identity(path)?;
    if !file.metadata()?.is_dir() {
        return Err(io::Error::other("path is not a directory"));
    }
    Ok(file)
}
