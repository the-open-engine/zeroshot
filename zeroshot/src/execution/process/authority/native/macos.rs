use std::io;
use std::mem::{MaybeUninit, size_of};

use super::ProcessIdentity;

// Public <sys/proc_info.h> flavor and complete ABI, not yet exposed by locked libc.
const PROC_PIDT_SHORTBSDINFO: libc::c_int = 13;
#[repr(C)]
struct BsdShortInfo {
    pid: u32,
    parent: u32,
    group: u32,
    _status: u32,
    _comm: [libc::c_char; libc::MAXCOMLEN],
    _flags: u32,
    _uid: libc::uid_t,
    _gid: libc::gid_t,
    _ruid: libc::uid_t,
    _rgid: libc::gid_t,
    _svuid: libc::uid_t,
    _svgid: libc::gid_t,
    _reserved: u32,
}
const _: () = assert!(size_of::<BsdShortInfo>() == 64);

pub(crate) fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
    let native_pid = i32::try_from(pid).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "process ID exceeds the native BSD range",
        )
    })?;
    let born = process_birth(native_pid)?;
    let mut info = MaybeUninit::<BsdShortInfo>::zeroed();
    let count = unsafe {
        libc::proc_pidinfo(
            native_pid,
            PROC_PIDT_SHORTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size_of::<BsdShortInfo>() as i32,
        )
    };
    if count <= 0 {
        return Err(last_error());
    }
    if count as usize != size_of::<BsdShortInfo>() {
        return Err(invalid_info());
    }
    let info = unsafe { info.assume_init() };
    // Bracket the ancestry query with kernel creation times to reject PID reuse.
    if info.pid != pid || process_birth(native_pid)? != born {
        return Err(invalid_info());
    }
    Ok(ProcessIdentity {
        pid,
        parent: info.parent,
        group: info.group,
        born,
    })
}

fn process_birth(pid: libc::pid_t) -> io::Result<u64> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    // The kernel returns a complete kinfo_proc, or an error if this bound cannot hold it.
    let mut bytes = [0_u8; 4096];
    let mut length = bytes.len();
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return Err(last_error());
    }
    if length == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "native process is no longer present",
        ));
    }
    parse_birth(bytes.get(..length).ok_or_else(invalid_info)?)
}

fn parse_birth(bytes: &[u8]) -> io::Result<u64> {
    if bytes.len() < size_of::<libc::timeval>() {
        return Err(invalid_info());
    }
    // Public kinfo_proc starts with extern_proc's start-time union at offset zero.
    let start = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<libc::timeval>()) };
    let seconds = u64::try_from(start.tv_sec).map_err(|_| invalid_info())?;
    let micros = u64::try_from(start.tv_usec).map_err(|_| invalid_info())?;
    if micros >= 1_000_000 {
        return Err(invalid_info());
    }
    seconds
        .checked_mul(1_000_000)
        .and_then(|seconds| seconds.checked_add(micros))
        .filter(|born| *born != 0)
        .ok_or_else(invalid_info)
}

fn last_error() -> io::Error {
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        io::Error::new(
            io::ErrorKind::NotFound,
            "native process is no longer present",
        )
    } else {
        error
    }
}

fn invalid_info() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid native BSD process identity",
    )
}

#[cfg(test)]
mod tests;
