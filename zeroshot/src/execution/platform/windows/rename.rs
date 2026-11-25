//! Atomic sibling publication compatible with retained Windows directory guards.

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path};

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_READ_ATTRIBUTES, SYNCHRONIZE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_SHARE_DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

pub(crate) fn commit_file(temporary: &Path, destination: &Path, parent: &Path) -> io::Result<()> {
    if temporary.parent() != Some(parent) || destination.parent() != Some(parent) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file publication requires one directory",
        ));
    }
    let name = destination
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "expected one file name"))?;
    rename_sibling(temporary, name, true)
}

/// A simple native rename stays in the source's parent. A full Win32 destination path reopens
/// that parent for FILE_WRITE_DATA, conflicting with the write/delete-denying directory guards.
/// Keep the guards and avoid that reopen; do not relax sharing to make publication succeed.
pub(crate) fn rename_sibling(source: &Path, name: &OsStr, replace: bool) -> io::Result<()> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected one file name",
        ));
    }
    let file = OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(source)?;
    super::file_identity(&file)?;
    let name: Vec<u16> = name.encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file name contains NUL",
        ));
    }
    let length = std::mem::size_of::<FILE_RENAME_INFORMATION>() + name.len() * 2;
    let length = u32::try_from(length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "file name exceeds limit"))?;
    // usize storage has the structure's pointer alignment; the extra tail holds the UTF-16 name.
    let mut buffer = vec![0_usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: the initialized, aligned buffer includes the complete name and remains live through
    // this synchronous call. RootDirectory stays NULL and only a simple name is permitted.
    let result = unsafe {
        (*info).Anonymous.ReplaceIfExists = replace;
        (*info).FileNameLength = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status,
            info.cast(),
            length,
            FileRenameInformation,
        )
    };
    if result < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    Ok(())
}
