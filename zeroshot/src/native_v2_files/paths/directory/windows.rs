use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;

use super::super::super::{Result, invalid};
use crate::execution::platform::{self, FileAccess};
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_READ_ATTRIBUTES, SYNCHRONIZE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_SHARE_DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

#[derive(Clone)]
pub(crate) struct Directory(Arc<PinnedDirectory>);
struct PinnedDirectory {
    path: PathBuf,
    guards: Vec<File>,
}

fn component(name: &OsStr) -> Result<()> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(invalid("expected one file name"));
    }
    Ok(())
}

impl Directory {
    pub fn open(path: &Path) -> Result<Self> {
        let path = std::path::absolute(path)?;
        super::super::reject_links(&path)?;
        let guards = super::super::guard_ancestors(&path)?;
        let file = platform::open_directory(&path)?;
        let last = guards
            .last()
            .ok_or_else(|| invalid("missing directory guard"))?;
        if platform::file_identity(&file)? != platform::file_identity(last)? {
            return Err(invalid("directory changed during inspection"));
        }
        Ok(Self(Arc::new(PinnedDirectory { path, guards })))
    }

    pub fn validate_private(&self) -> Result<()> {
        platform::validate_private_file(
            self.0
                .guards
                .last()
                .ok_or_else(|| invalid("missing directory guard"))?,
        )
    }

    pub fn child(&self, name: &OsStr) -> Result<Self> {
        component(name)?;
        Self::open(&self.0.path.join(name))
    }

    pub fn create_directory(&self, name: &OsStr) -> Result<Self> {
        component(name)?;
        platform::create_private_directory(&self.0.path.join(name))?;
        self.child(name)
    }

    pub fn create_file(&self, name: &OsStr) -> Result<File> {
        component(name)?;
        platform::private_file(&self.0.path.join(name), FileAccess::CreateNew)
    }

    pub fn read_file(&self, name: &OsStr) -> Result<File> {
        component(name)?;
        let file = platform::private_file(&self.0.path.join(name), FileAccess::Read)?;
        super::super::regular(&file)?;
        Ok(file)
    }

    pub fn writable_file(&self, name: &OsStr) -> Result<File> {
        component(name)?;
        super::super::writable_state_file(&self.0.path.join(name))
    }

    pub fn entries(&self) -> Result<impl Iterator<Item = Result<OsString>>> {
        Ok(std::fs::read_dir(&self.0.path)?.map(|entry| entry.map(|entry| entry.file_name())))
    }

    pub fn remove_file(&self, name: &OsStr) -> Result<()> {
        component(name)?;
        std::fs::remove_file(self.0.path.join(name))
    }

    pub fn remove_tree(&self, name: &OsStr) -> Result<()> {
        component(name)?;
        let path = self.0.path.join(name);
        if std::fs::symlink_metadata(&path)?.is_dir() {
            let child = self.child(name)?;
            for entry in child.entries()? {
                child.remove_tree(&entry?)?;
            }
            drop(child);
            std::fs::remove_dir(path)
        } else {
            self.remove_file(name)
        }
    }

    pub fn rename_noreplace(&self, source: &OsStr, destination: &Self, name: &OsStr) -> Result<()> {
        component(source)?;
        component(name)?;
        if self.0.path != destination.0.path {
            return Err(invalid("file publication requires one directory"));
        }
        rename(&self.0.path.join(source), name, false)
    }

    pub fn rename_replace(&self, source: &OsStr, destination: &OsStr) -> Result<()> {
        component(source)?;
        component(destination)?;
        rename(&self.0.path.join(source), destination, true)
    }
}

/// A simple native rename stays in the source's parent. A full Win32 destination path reopens
/// that parent for FILE_WRITE_DATA, conflicting with the write/delete-denying directory guards.
/// Keep the guards and avoid that reopen; do not relax sharing to make publication succeed.
fn rename(source: &Path, name: &OsStr, replace: bool) -> Result<()> {
    let file = OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(source)?;
    platform::file_identity(&file)?;
    let name: Vec<u16> = name.encode_wide().collect();
    if name.contains(&0) {
        return Err(invalid("file name contains NUL"));
    }
    let length = std::mem::size_of::<FILE_RENAME_INFORMATION>() + name.len() * 2;
    let length = u32::try_from(length).map_err(|_| invalid("file name exceeds limit"))?;
    // usize storage has the structure's pointer alignment; the extra tail holds the UTF-16 name.
    let mut buffer = vec![0_usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: the initialized, aligned buffer includes the complete name and remains live through
    // this synchronous call. RootDirectory stays NULL and component() permits only a simple name.
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
