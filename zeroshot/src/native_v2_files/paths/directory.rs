//! Directory capabilities used after path validation, including deferred cleanup.

use std::ffi::OsString;
use std::fs::File;

use super::super::Result;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub(crate) use unix::Directory;
#[cfg(windows)]
pub(crate) use windows::Directory;

pub(crate) struct TemporaryFile {
    pub file: File,
    directory: Directory,
    name: OsString,
}

impl TemporaryFile {
    pub fn new(directory: &Directory) -> Result<Self> {
        let name = OsString::from(format!("stage-{}", uuid::Uuid::now_v7()));
        Self::named(directory, name)
    }

    pub fn named(directory: &Directory, name: OsString) -> Result<Self> {
        Ok(Self {
            file: directory.create_file(&name)?,
            directory: directory.clone(),
            name,
        })
    }

    pub fn commit(&self, name: &std::ffi::OsStr) -> Result<()> {
        self.file.sync_all()?;
        self.directory.rename_replace(&self.name, name)
    }

    pub fn commit_noreplace(&self, name: &std::ffi::OsStr) -> Result<()> {
        self.file.sync_all()?;
        self.directory
            .rename_noreplace(&self.name, &self.directory, name)
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = self.directory.remove_file(&self.name);
    }
}
