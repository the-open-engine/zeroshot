use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::os::windows::ffi::OsStrExt;

use super::super::super::{Result, invalid};
use crate::execution::platform::{self, FileAccess};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

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
        rename(
            &self.0.path.join(source),
            &destination.0.path.join(name),
            false,
        )
    }

    pub fn rename_replace(&self, source: &OsStr, destination: &OsStr) -> Result<()> {
        component(source)?;
        component(destination)?;
        rename(
            &self.0.path.join(source),
            &self.0.path.join(destination),
            true,
        )
    }
}

fn rename(source: &Path, destination: &Path, replace: bool) -> Result<()> {
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    // Both pinned parents disallow rename/reparse mutation while these paths are in use.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
