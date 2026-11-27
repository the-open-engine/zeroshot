use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::os::windows::fs::OpenOptionsExt;
use std::sync::Arc;

use super::super::super::{Result, invalid};
use crate::execution::platform::{self, FileAccess};

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

fn clear_readonly(path: &Path) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_WRITE_ATTRIBUTES,
    };
    // The caller holds the parent guards. Pin this entry without following reparse points;
    // deny replacement while repairing only the disposable copy's attributes.
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    platform::file_identity(&file)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() {
        super::super::regular(&file)?;
    }
    let mut permissions = metadata.permissions();
    #[expect(
        clippy::permissions_set_readonly_false,
        reason = "Windows disposable copy only"
    )]
    permissions.set_readonly(false);
    file.set_permissions(permissions)
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
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.permissions().readonly() {
            clear_readonly(&path)?;
        }
        if metadata.is_dir() {
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
        platform::windows::rename_in_parent(&self.0.path.join(source), name, false)
    }

    pub fn rename_replace(&self, source: &OsStr, destination: &OsStr) -> Result<()> {
        component(source)?;
        component(destination)?;
        platform::windows::rename_in_parent(&self.0.path.join(source), destination, true)
    }
}
