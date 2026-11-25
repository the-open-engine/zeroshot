use std::fs::File;
use std::io;
use std::path::{Component, Path};

use super::{Result, invalid};
use crate::execution::platform;

mod directory;
pub(super) use directory::{Directory, TemporaryFile};

/// Check every existing ancestor, including Windows junctions. Host-private parents prevent
/// untrusted workers from swapping internal catalog/blob paths after this validation.
pub(super) fn reject_links(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(invalid("filesystem links are not permitted"));
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err(invalid("filesystem reparse points are not permitted"));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(super) fn regular(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("expected a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(invalid("hard links are not permitted"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.nNumberOfLinks != 1 {
            return Err(invalid("hard links are not permitted"));
        }
    }
    Ok(())
}

pub(super) fn private_root(path: &Path) -> Result<()> {
    reject_links(path)?;
    if !path.exists() {
        platform::private_directory(path)?;
    }
    reject_links(path)?;
    platform::ensure_private_directory(path)
}

/// Open every Unix component relative to the previous pinned directory, refusing links. On
/// Windows the platform opener rejects reparse points and the complete ancestry is checked.
pub(super) fn open_source(path: &Path) -> Result<File> {
    let absolute = std::path::absolute(path)?;
    reject_links(&absolute)?;
    #[cfg(unix)]
    {
        let mut file = platform::open_directory(Path::new("/"))?;
        let mut components = absolute.components().peekable();
        while let Some(component) = components.next() {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(name) => {
                    file = open_component(&file, name, components.peek().is_some())?
                }
                _ => return Err(invalid("source path must not contain parent traversal")),
            }
        }
        Ok(file)
    }
    #[cfg(windows)]
    {
        let _ = Component::CurDir;
        let file = platform::open_readonly_file(&absolute)
            .or_else(|_| platform::open_directory(&absolute))?;
        platform::file_identity(&file)?;
        reject_links(&absolute)?;
        Ok(file)
    }
}

#[cfg(unix)]
pub(super) fn open_child(parent: &File, name: &std::ffi::OsStr) -> Result<File> {
    open_component(parent, name, false)
}

#[cfg(unix)]
fn open_component(parent: &File, name: &std::ffi::OsStr, search: bool) -> Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| invalid("invalid file name"))?;
    #[cfg(target_os = "linux")]
    let directory_flags = libc::O_PATH | libc::O_DIRECTORY;
    #[cfg(target_os = "macos")]
    let directory_flags = libc::O_SEARCH;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let directory_flags = libc::O_RDONLY | libc::O_DIRECTORY;
    let access_flags = if search {
        directory_flags
    } else {
        libc::O_RDONLY | libc::O_NONBLOCK
    };
    // SAFETY: parent and the NUL-terminated single component remain live throughout openat.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            access_flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the new descriptor has exactly one owner.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Windows handles without write/delete sharing keep each checked ancestor from becoming a junction
/// between checking it and opening a member. Unix member opens are descriptor-relative instead.
pub(super) fn guard_ancestors(path: &Path) -> Result<Vec<File>> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, GENERIC_READ};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, OPEN_EXISTING, FILE_SHARE_READ, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT,
        };
        let absolute = std::path::absolute(path)?;
        let mut guards = Vec::new();
        let mut ancestors: Vec<_> = absolute.ancestors().collect();
        ancestors.reverse();
        for ancestor in ancestors {
            if !ancestor.is_dir() {
                continue;
            }
            let wide: Vec<_> = ancestor.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: wide is a terminated path; the returned handle is owned exactly once.
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    GENERIC_READ,
                    FILE_SHARE_READ,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                    std::ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let file = unsafe { File::from_raw_handle(handle) };
            platform::file_identity(&file)?;
            guards.push(file);
        }
        Ok(guards)
    }
    #[cfg(unix)]
    {
        let _ = path;
        Ok(Vec::new())
    }
}

#[cfg(windows)]
pub(super) fn writable_state_file(path: &Path) -> Result<File> {
    match platform::private_file(path, platform::FileAccess::CreateNew) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            // Validate before an opener that can repair permissions: a hard link must not cause
            // a rejected operation to alter another file's security.
            let existing = platform::private_file(path, platform::FileAccess::Read)?;
            regular(&existing)?;
            platform::private_file(path, platform::FileAccess::ReadWrite)
        }
        Err(error) => Err(error),
    }
}
