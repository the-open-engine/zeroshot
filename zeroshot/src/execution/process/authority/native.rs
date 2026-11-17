use std::io;
#[cfg(not(windows))]
use std::path::PathBuf;

/// Native creation identities are opaque and comparable only on the same OS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProcessIdentity {
    pub pid: u32,
    /// Native parent; verified Windows human ancestry boundaries report zero.
    pub parent: u32,
    /// Native process group, or zero on Windows.
    pub group: u32,
    pub born: u64,
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
use windows as platform;
#[cfg(windows)]
pub(super) use windows::{outside_jobs, registry_directory, root_born};

#[cfg(not(windows))]
pub(super) fn outside_jobs(_caller: ProcessIdentity) -> io::Result<bool> {
    Ok(false)
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub(super) fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
    if pid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid process ID",
        ));
    }
    platform::process_identity(pid)
}

#[cfg(unix)]
pub(super) fn registry_directory() -> io::Result<PathBuf> {
    let root = if cfg!(target_os = "macos") {
        "/private/tmp"
    } else {
        "/tmp"
    };
    // The native effective user, rather than inherited path variables, selects the registry.
    let user = unsafe { libc::geteuid() };
    Ok(PathBuf::from(root).join(format!("zeroshot-process-authority-{user}")))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) fn process_identity(_pid: u32) -> io::Result<ProcessIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native process identity is unavailable on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
pub(super) fn registry_directory() -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native process authority storage is unavailable on this platform",
    ))
}
