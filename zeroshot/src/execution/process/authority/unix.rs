//! Allocation-free publication in the existing pre-exec child, before native provider code.
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;

pub(super) fn configure(
    command: &mut tokio::process::Command,
    directory: &Path,
    pending: &str,
    file: &File,
) -> io::Result<()> {
    let directory = crate::execution::platform::open_directory(directory)?;
    let directory = File::from(crate::execution::platform::retained_descriptor(&directory)?);
    let pending = CString::new(pending)?;
    let record = file.as_raw_fd();
    // SAFETY: only fixed stack buffers, inherited descriptors and direct syscalls are used.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            let pid = libc::getpid() as u64;
            let bytes = pid.to_le_bytes();
            if libc::pwrite(record, bytes.as_ptr().cast(), bytes.len(), 0) != bytes.len() as isize {
                return Err(io::Error::last_os_error());
            }
            let mut name = [0_u8; 21];
            let mut position = name.len() - 1;
            let mut remaining = pid;
            loop {
                position -= 1;
                name[position] = b'0' + (remaining % 10) as u8;
                remaining /= 10;
                if remaining == 0 {
                    break;
                }
            }
            if libc::renameat(
                directory.as_raw_fd(),
                pending.as_ptr(),
                directory.as_raw_fd(),
                name[position..].as_ptr().cast(),
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}
