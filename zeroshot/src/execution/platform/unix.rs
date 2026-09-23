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
