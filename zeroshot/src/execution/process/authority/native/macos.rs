use std::io;
use std::mem::{MaybeUninit, size_of};

use super::ProcessIdentity;

pub(crate) fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
    let native_pid = i32::try_from(pid).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "process ID exceeds the native BSD range",
        )
    })?;
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    // The BSD-info flavor reads fixed-size process metadata, never argv or environment.
    let count = unsafe {
        libc::proc_pidinfo(
            native_pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size_of::<libc::proc_bsdinfo>() as i32,
        )
    };
    if count <= 0 {
        let error = io::Error::last_os_error();
        return Err(if error.raw_os_error() == Some(libc::ESRCH) {
            io::Error::new(
                io::ErrorKind::NotFound,
                "native process is no longer present",
            )
        } else {
            error
        });
    }
    if count as usize != size_of::<libc::proc_bsdinfo>() {
        return Err(invalid_info());
    }
    // A complete fixed-size native result initializes every field we inspect.
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid || info.pbi_start_tvusec >= 1_000_000 {
        return Err(invalid_info());
    }
    let born = info
        .pbi_start_tvsec
        .checked_mul(1_000_000)
        .and_then(|seconds| seconds.checked_add(info.pbi_start_tvusec))
        .ok_or_else(invalid_info)?;
    Ok(ProcessIdentity {
        pid,
        parent: info.pbi_ppid,
        group: info.pbi_pgid,
        born,
    })
}

fn invalid_info() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid native BSD process identity",
    )
}
