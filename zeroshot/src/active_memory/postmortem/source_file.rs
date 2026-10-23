//! Current-source evidence is read through pinned, regular-file descriptors.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub(super) fn open(root: &Path, relative: &str) -> io::Result<File> {
    crate::active_memory::validate_relative_path(relative)
        .map_err(|_| io::Error::other("invalid source reference"))?;
    let root = root.canonicalize()?;
    let directory = crate::execution::platform::open_directory(&root)?;
    let path = root.join(relative).canonicalize()?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| io::Error::other("source reference escapes the workspace"))?;
    let file = open_beneath(&directory, &root, relative)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("source reference is not a regular file"));
    }
    Ok(file)
}

#[cfg(unix)]
fn open_beneath(directory: &File, _root: &Path, relative: &Path) -> io::Result<File> {
    let mut directory = directory.try_clone()?;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let std::path::Component::Normal(name) = component else {
            return Err(io::Error::other("invalid source component"));
        };
        let file = open_component(&directory, name, components.peek().is_some())?;
        if components.peek().is_none() {
            return Ok(file);
        }
        directory = file;
    }
    Err(io::Error::other("source reference is a directory"))
}

#[cfg(unix)]
fn open_component(directory: &File, name: &std::ffi::OsStr, parent: bool) -> io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let name = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| io::Error::other("invalid source component"))?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if parent { libc::O_DIRECTORY } else { 0 };
    // SAFETY: directory remains open, name is NUL-terminated, and no creation mode is used.
    let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new descriptor transferred to File exactly once.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(not(unix))]
fn open_beneath(directory: &File, root: &Path, relative: &Path) -> io::Result<File> {
    use crate::execution::platform::{open_identity, open_readonly_file};

    let path = root.join(relative);
    let expected = open_identity(&path)?;
    let file = open_readonly_file(&path)?;
    same_identity(&file, &expected)?;
    same_identity(directory, &open_identity(root)?)?;
    if path.canonicalize()? != path {
        return Err(io::Error::other(
            "source reference changed during inspection",
        ));
    }
    same_identity(&file, &open_identity(&path)?)?;
    Ok(file)
}

#[cfg(not(unix))]
fn same_identity(first: &File, second: &File) -> io::Result<()> {
    use crate::execution::platform::file_identity;
    if file_identity(first)? != file_identity(second)? {
        return Err(io::Error::other(
            "source reference changed during inspection",
        ));
    }
    Ok(())
}

pub(super) fn has_identifier(file: File, identifiers: &[String]) -> bool {
    let mut text = String::new();
    if file.take(256 * 1024).read_to_string(&mut text).is_err() {
        return false;
    }
    let tokens = text
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .collect::<BTreeSet<_>>();
    identifiers
        .iter()
        .any(|identifier| tokens.contains(identifier.as_str()))
}

#[cfg(test)]
mod tests;
