//! Machine-owned evidence only: SQLite messages can contain SQL, triggers and payloads.

use std::{fmt, io};

/// Safe evidence from one failed ledger operation, independent of its recovery attempts.
#[derive(Clone, Debug, PartialEq)]
pub struct SqliteStorageError {
    pub code: rusqlite::ffi::Error,
    /// SQLite's native OS error, when still attributable to the failed operation.
    pub system_error: Option<i32>,
    /// Available bytes on the ledger's filesystem, measured only after failure.
    pub available_bytes: Option<u64>,
}

impl From<rusqlite::ffi::Error> for SqliteStorageError {
    fn from(code: rusqlite::ffi::Error) -> Self {
        Self {
            code,
            system_error: None,
            available_bytes: None,
        }
    }
}

impl fmt::Display for SqliteStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.code)?;
        if let Some(code) = self.system_error {
            write!(
                formatter,
                ". Native OS cause: {}",
                io::Error::from_raw_os_error(code)
            )?;
        }
        if let Some(bytes) = self.available_bytes {
            write!(
                formatter,
                ". Available space on the storage volume: {bytes} B"
            )?;
        }
        if self.code.code == rusqlite::ffi::ErrorCode::DiskFull
            || self.available_bytes == Some(0)
            || self.system_error.is_some_and(|code| {
                io::Error::from_raw_os_error(code).kind() == io::ErrorKind::StorageFull
            })
        {
            write!(
                formatter,
                ". Free space or select a larger run size before resuming"
            )?;
        }
        Ok(())
    }
}
