//! Use SQLite's native VFS locks, including its same-process lock bookkeeping on Unix.
use std::io::Write;

use rusqlite::{Connection, ffi};

use super::RunLedgerError;

pub(super) struct SharedReadLock<'a> {
    file: *mut ffi::sqlite3_file,
    methods: &'a ffi::sqlite3_io_methods,
    _connection: &'a mut Connection,
}

impl<'a> SharedReadLock<'a> {
    pub(super) fn acquire(connection: &'a mut Connection) -> Result<Self, RunLedgerError> {
        let mut file = std::ptr::null_mut::<ffi::sqlite3_file>();
        // SAFETY: SQLite owns this file for the lifetime of the borrowed, idle connection.
        // FILE_POINTER writes one sqlite3_file pointer to the correctly typed out parameter.
        check(unsafe {
            ffi::sqlite3_file_control(
                connection.handle(),
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_FILE_POINTER,
                std::ptr::from_mut(&mut file).cast(),
            )
        })?;
        // SAFETY: after successful FILE_POINTER the pointer and vtable belong to this connection.
        let methods = unsafe { file.as_ref().and_then(|file| file.pMethods.as_ref()) }
            .ok_or(RunLedgerError::Storage)?;
        let lock = methods.xLock.ok_or(RunLedgerError::Storage)?;
        // SAFETY: the native VFS supports shared locks on read-only database handles.
        check(unsafe { lock(file, ffi::SQLITE_LOCK_SHARED) })?;
        Ok(Self {
            file,
            methods,
            _connection: connection,
        })
    }

    pub(super) fn require_no_writer(&self) -> Result<(), RunLedgerError> {
        let check_reserved = self
            .methods
            .xCheckReservedLock
            .ok_or(RunLedgerError::Storage)?;
        let mut reserved = 0;
        // SAFETY: this borrowed native file remains open and the out parameter is initialized.
        check(unsafe { check_reserved(self.file, &mut reserved) })?;
        if reserved == 0 {
            Ok(())
        } else {
            check(ffi::SQLITE_BUSY)
        }
    }

    pub(super) fn copy_database(&self, output: &mut impl Write) -> Result<(), RunLedgerError> {
        let size = self.methods.xFileSize.ok_or(RunLedgerError::Storage)?;
        let read = self.methods.xRead.ok_or(RunLedgerError::Storage)?;
        let mut length = 0;
        // SAFETY: the native file and initialized out parameter remain valid under this guard.
        check(unsafe { size(self.file, &mut length) })?;
        if length < 0 {
            return Err(RunLedgerError::Corrupt);
        }
        let mut buffer = vec![0u8; 64 * 1024];
        let mut offset = 0;
        while offset < length {
            let count = (length - offset).min(buffer.len() as i64) as i32;
            // SAFETY: the buffer holds count bytes, offset is nonnegative and below file size.
            check(unsafe { read(self.file, buffer.as_mut_ptr().cast(), count, offset) })?;
            output
                .write_all(&buffer[..count as usize])
                .map_err(|_| RunLedgerError::Storage)?;
            offset += i64::from(count);
        }
        Ok(())
    }
}

impl Drop for SharedReadLock<'_> {
    fn drop(&mut self) {
        if let Some(unlock) = self.methods.xUnlock {
            // SAFETY: the connection outlives this guard; no pager operation ran while it held
            // the native lock. Closing the connection also releases locks after an I/O failure.
            let _ = unsafe { unlock(self.file, ffi::SQLITE_LOCK_NONE) };
        }
    }
}

fn check(code: i32) -> Result<(), RunLedgerError> {
    if code == ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(RunLedgerError::SqliteStorage(ffi::Error::new(code)))
    }
}
