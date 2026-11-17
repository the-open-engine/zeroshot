use std::io;

use super::ProcessIdentity;

unsafe extern "C" {
    fn zeroshot_macos_process_identity(
        pid: libc::pid_t,
        parent: *mut u32,
        group: *mut u32,
        born: *mut u64,
    ) -> libc::c_int;
}

pub(crate) fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
    let native_pid = i32::try_from(pid).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "process ID exceeds the native BSD range",
        )
    })?;
    let (mut parent, mut group, mut born) = (0, 0, 0);
    // The SDK shim validates one complete kernel record before initializing these outputs.
    let status =
        unsafe { zeroshot_macos_process_identity(native_pid, &mut parent, &mut group, &mut born) };
    if status != 0 {
        return Err(match status {
            -1 => io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid native BSD process identity",
            ),
            libc::ESRCH => io::Error::new(
                io::ErrorKind::NotFound,
                "native process is no longer present",
            ),
            error => io::Error::from_raw_os_error(error),
        });
    }
    Ok(ProcessIdentity {
        pid,
        parent,
        group,
        born,
    })
}

#[cfg(test)]
mod tests;
