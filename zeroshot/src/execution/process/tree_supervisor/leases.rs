//! Tracks only live workspace lease descriptors for the local command subreaper's fork.
//! No process identity, persistent registry, credentials or public recovery state is introduced.
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct Entry {
    file: File,
    workspace: PathBuf,
}
static LEASES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
pub(crate) struct WorkspaceLease(i32);
impl WorkspaceLease {
    pub(crate) fn retain(file: &File, workspace: &Path) -> io::Result<Self> {
        let file = file.try_clone()?;
        let key = file.as_raw_fd();
        LEASES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Entry {
                file,
                workspace: workspace.to_owned(),
            });
        Ok(Self(key))
    }
}
impl Drop for WorkspaceLease {
    fn drop(&mut self) {
        LEASES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|entry| entry.file.as_raw_fd() != self.0);
    }
}
pub(super) fn capture(workspace: &Path) -> io::Result<Vec<File>> {
    LEASES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|entry| workspace.starts_with(&entry.workspace))
        .map(|entry| super::retained_descriptor(&entry.file).map(File::from))
        .collect()
}
