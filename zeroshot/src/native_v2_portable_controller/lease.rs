use std::fs::File;
use crate::execution::platform::{self, FileAccess, FileIdentity};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;
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
}

impl ControllerLease {
    pub fn acquire(path: impl Into<PathBuf>) -> Result<Self, ControllerLeaseError> {
        let path = path.into();
        let parent = path.parent().ok_or(ControllerLeaseError::StateDirectory)?;
        prepare_state_directory(parent)?;
        reject_non_file(&path)?;
        let file = platform::private_file(&path, FileAccess::ReadWrite)
            .map_err(|_| ControllerLeaseError::StateDirectory)?;
        let metadata = file
            .metadata()
            .map_err(|_| ControllerLeaseError::InvalidPath)?;
        if !metadata.is_file() {
            return Err(ControllerLeaseError::InvalidPath);
        }
        file.try_lock_exclusive()
            .map_err(|_| ControllerLeaseError::Held)?;
        Ok(Self {
            identity: platform::file_identity(&file)
                .map_err(|_| ControllerLeaseError::InvalidPath)?,
            file,
            path,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn is_intact(&self) -> bool {
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

    /// Checks whether an existing private lease is owned without creating or changing it.
    #[cfg(feature = "ui")]
    pub(crate) fn is_held(path: &Path) -> Result<bool, ControllerLeaseError> {
        reject_non_file(path)?;
        let file = platform::private_file(path, FileAccess::ReadWriteExisting)
            .map_err(|_| ControllerLeaseError::InvalidPath)?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                FileExt::unlock(&file).map_err(|_| ControllerLeaseError::InvalidPath)?;
                Ok(false)
            }
            Err(error) if lock_is_contended(&error) => Ok(true),
            Err(_) => Err(ControllerLeaseError::InvalidPath),
        }
    }
}

#[cfg(feature = "ui")]
fn lock_is_contended(error: &io::Error) -> bool {
    if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
        return true;
    }
    #[cfg(windows)]
    {
        // LockFileEx can report an overlapping exclusive lock as asynchronous contention.
        error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_IO_PENDING as i32)
    }
    #[cfg(not(windows))]
    false
}

#[cfg(all(test, feature = "ui", windows))]
#[test]
fn windows_lock_probe_accepts_only_contention_errors() {
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_IO_PENDING, ERROR_LOCK_VIOLATION};

    assert!(lock_is_contended(&io::Error::from_raw_os_error(
        ERROR_LOCK_VIOLATION as i32,
    )));
    assert!(lock_is_contended(&io::Error::from_raw_os_error(
        ERROR_IO_PENDING as i32,
    )));
    assert!(!lock_is_contended(&io::Error::from_raw_os_error(
        ERROR_ACCESS_DENIED as i32,
    )));
}

impl std::fmt::Debug for ControllerLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControllerLease")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
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
