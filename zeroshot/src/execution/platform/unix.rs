use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use super::FileAccess;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileIdentity {
    device: u64,
    inode: u64,
}

pub(crate) fn file_identity(file: &File) -> io::Result<FileIdentity> {
    let metadata = file.metadata()?;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

pub(crate) fn open_identity(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

pub(crate) fn open_readonly_file(path: &Path) -> io::Result<File> {
    let file = open_identity(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("path is not a regular file"));
    }
    Ok(file)
}

pub(crate) fn private_directory(path: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let file = open_directory(path)?;
    if !file.metadata()?.is_dir() {
        return Err(io::Error::other("private path is not a directory"));
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o700))
}

pub(crate) fn private_file(path: &Path, access: FileAccess) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(!matches!(access, FileAccess::Read))
        .create(matches!(access, FileAccess::ReadWrite))
        .create_new(matches!(access, FileAccess::CreateNew))
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .mode(0o600);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("private path is not a file"));
    }
    if access.repairs_security() {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    validate_private_file(&file)?;
    Ok(file)
}

pub(crate) fn validate_private_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "file is not private to this user",
        ));
    }
    Ok(())
}

pub(crate) fn commit_file(temporary: &Path, destination: &Path, parent: &Path) -> io::Result<()> {
    std::fs::rename(temporary, destination)?;
    File::open(parent)?.sync_all()
}

pub(crate) fn create_private_directory(path: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}

pub(crate) type ControllerChild = tokio::process::Child;

pub(crate) fn guard_controller_console() -> io::Result<()> {
    Ok(())
}

/// Register before bootstrap consumption so a termination during startup is retained too.
pub(crate) fn controller_termination() -> io::Result<impl Future<Output = ()> + Send> {
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    Ok(async move {
        signal.recv().await;
    })
}

pub(crate) fn spawn_controller(
    command: &mut tokio::process::Command,
) -> io::Result<ControllerChild> {
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()
}

/// Stops a controller that never became ready, with every process left in its process group such
/// as Git, and waits for the controller itself to exit. Restic runs in its own contained group,
/// which the controller stops at its preparation deadline.
pub(crate) async fn stop_controller(child: &mut ControllerChild) -> io::Result<()> {
    // `spawn_controller` made the controller its session's process group leader. Its identifier
    // stays reserved until the controller is reaped below.
    if let Some(group) = child.id().and_then(|id| libc::pid_t::try_from(id).ok()) {
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
    child.kill().await
}
