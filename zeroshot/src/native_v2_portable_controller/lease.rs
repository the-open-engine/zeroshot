use std::fs::File;
use crate::execution::platform::{self, FileAccess, FileIdentity};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ControllerLeaseError {
    #[error("controller state directory is unavailable")]
    StateDirectory,
    #[error("controller lease path is not a regular file")]
    InvalidPath,
    #[error("another controller owns this lease")]
    Held,
}

/// Process-lifetime, filesystem-exclusive ownership for one portable run controller.
pub struct ControllerLease {
    file: File,
    path: PathBuf,
    identity: FileIdentity,
    ancestors: Vec<Self>,
    #[cfg(target_os = "linux")]
    process_lease: Option<crate::execution::process::tree_supervisor::WorkspaceLease>,
}

impl ControllerLease {
    pub fn acquire(path: impl Into<PathBuf>) -> Result<Self, ControllerLeaseError> {
        Self::acquire_mode(path.into(), true)
    }

    pub(crate) fn acquire_workspace(
        path: &Path,
        workspace: &Path,
    ) -> Result<Self, ControllerLeaseError> {
        let root = path.parent().ok_or(ControllerLeaseError::StateDirectory)?;
        let mut ancestors = Vec::new();
        let parents: Vec<_> = workspace.ancestors().skip(1).collect();
        for parent in parents.into_iter().rev() {
            ancestors.push(Self::acquire_mode(
                workspace_lease_path(root, parent),
                false,
            )?);
        }
        let mut lease = Self::acquire(path)?;
        #[cfg(target_os = "linux")]
        for guard in ancestors.iter_mut().chain(std::iter::once(&mut lease)) {
            guard.process_lease = Some(
                crate::execution::process::tree_supervisor::WorkspaceLease::retain(
                    &guard.file,
                    workspace,
                )
                .map_err(|_| ControllerLeaseError::InvalidPath)?,
            );
        }
        lease.ancestors = ancestors;
        Ok(lease)
    }

    fn acquire_mode(path: PathBuf, exclusive: bool) -> Result<Self, ControllerLeaseError> {
        let parent = path.parent().ok_or(ControllerLeaseError::StateDirectory)?;
        prepare_state_directory(parent)?;
        reject_non_file(&path)?;
        let file = platform::private_file(&path, FileAccess::ReadWrite)
            .map_err(|_| ControllerLeaseError::StateDirectory)?;
        Self::lock_file(file, path, exclusive)
    }

    /// Claims the original retained lease without creating or repairing a path before confirmation.
    pub(crate) fn acquire_existing(
        path: &Path,
        expected: FileIdentity,
    ) -> Result<Self, ControllerLeaseError> {
        let file =
            platform::open_readonly_file(path).map_err(|_| ControllerLeaseError::InvalidPath)?;
        if !platform::file_identity(&file).is_ok_and(|identity| identity == expected) {
            return Err(ControllerLeaseError::InvalidPath);
        }
        let lease = Self::lock_file(file, path.to_owned(), true)?;
        if !lease.is_intact() {
            return Err(ControllerLeaseError::InvalidPath);
        }
        Ok(lease)
    }

    fn lock_file(file: File, path: PathBuf, exclusive: bool) -> Result<Self, ControllerLeaseError> {
        let metadata = file
            .metadata()
            .map_err(|_| ControllerLeaseError::InvalidPath)?;
        if !metadata.is_file() {
            return Err(ControllerLeaseError::InvalidPath);
        }
        if exclusive {
            file.try_lock_exclusive()
        } else {
            FileExt::try_lock_shared(&file)
        }
        .map_err(|_| ControllerLeaseError::Held)?;
        Ok(Self {
            identity: platform::file_identity(&file)
                .map_err(|_| ControllerLeaseError::InvalidPath)?,
            file,
            path,
            ancestors: Vec::new(),
            #[cfg(target_os = "linux")]
            process_lease: None,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn is_intact(&self) -> bool {
        if !self.ancestors.iter().all(Self::is_intact) {
            return false;
        }
        let Ok(path_metadata) = std::fs::symlink_metadata(&self.path) else {
            return false;
        };
        if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
            return false;
        }
        if !platform::file_identity(&self.file).is_ok_and(|identity| identity == self.identity) {
            return false;
        }
        platform::open_identity(&self.path)
            .and_then(|file| platform::file_identity(&file))
            .is_ok_and(|identity| identity == self.identity)
    }

    pub(crate) fn identity(&self) -> FileIdentity {
        self.identity
    }

    /// Probes an existing lease through a read-only handle, without claiming controller ownership.
    pub(crate) fn probe(path: &Path) -> Result<LeaseProbe, ControllerLeaseError> {
        let file =
            platform::open_readonly_file(path).map_err(|_| ControllerLeaseError::InvalidPath)?;
        let identity =
            platform::file_identity(&file).map_err(|_| ControllerLeaseError::InvalidPath)?;
        let held = match FileExt::try_lock_shared(&file) {
            Ok(()) => {
                FileExt::unlock(&file).map_err(|_| ControllerLeaseError::InvalidPath)?;
                false
            }
            Err(error) if lock_is_contended(&error) => true,
            Err(_) => return Err(ControllerLeaseError::InvalidPath),
        };
        if platform::open_identity(path)
            .and_then(|current| platform::file_identity(&current))
            .is_ok_and(|current| current == identity)
        {
            Ok(LeaseProbe { identity, held })
        } else {
            Err(ControllerLeaseError::InvalidPath)
        }
    }

    #[cfg(all(test, feature = "ui"))]
    pub(crate) fn is_held(path: &Path) -> Result<bool, ControllerLeaseError> {
        Self::probe(path).map(|probe| probe.held)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LeaseProbe {
    pub identity: FileIdentity,
    pub held: bool,
}

fn lock_is_contended(error: &io::Error) -> bool {
    if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
        return true;
    }
    #[cfg(windows)]
    {
        error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_IO_PENDING as i32)
    }
    #[cfg(not(windows))]
    false
}

impl Drop for ControllerLease {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        self.process_lease.take();
        // A fork-before-exec child can temporarily retain the same open file description.
        let _ = FileExt::unlock(&self.file);
    }
}

impl std::fmt::Debug for ControllerLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControllerLease")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

pub(crate) fn workspace_lease_path(root: &Path, workspace: &Path) -> PathBuf {
    let digest = Sha256::digest(workspace.as_os_str().as_encoded_bytes());
    root.join(format!("{digest:x}.lock"))
}

fn prepare_state_directory(path: &Path) -> Result<(), ControllerLeaseError> {
    platform::private_directory(path).map_err(|_| ControllerLeaseError::StateDirectory)
}

fn reject_non_file(path: &Path) -> Result<(), ControllerLeaseError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(ControllerLeaseError::InvalidPath),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ControllerLeaseError::InvalidPath),
    }
}

#[cfg(test)]
#[path = "lease/tests.rs"]
mod tests;
