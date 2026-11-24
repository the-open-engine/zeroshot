use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::sync::Arc;

use super::super::super::{Result, invalid};
use crate::execution::platform;

#[derive(Clone)]
pub(crate) struct Directory(Arc<File>);

fn component(name: &OsStr) -> Result<CString> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(invalid("expected one file name"));
    }
    CString::new(name.as_bytes()).map_err(|_| invalid("invalid file name"))
}

fn check(result: i32) -> Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn open_at(directory: RawFd, name: &CStr, flags: i32) -> Result<File> {
    // SAFETY: the borrowed directory and terminated name remain valid; this owns only the new fd.
    let fd = unsafe {
        libc::openat(
            directory,
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    check(fd)?;
    Ok(unsafe { File::from_raw_fd(fd) })
}

impl Directory {
    pub fn open(path: &Path) -> Result<Self> {
        let file = super::super::open_source(path)?;
        if !file.metadata()?.is_dir() {
            return Err(invalid("expected a directory"));
        }
        Ok(Self(Arc::new(file)))
    }

    pub fn validate_private(&self) -> Result<()> {
        platform::validate_private_file(&self.0)
    }

    pub fn child(&self, name: &OsStr) -> Result<Self> {
        let file = open_at(
            self.0.as_raw_fd(),
            &component(name)?,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        Ok(Self(Arc::new(file)))
    }

    pub fn create_directory(&self, name: &OsStr) -> Result<Self> {
        let name_c = component(name)?;
        // SAFETY: mkdirat is confined to the pinned parent and one validated component.
        check(unsafe { libc::mkdirat(self.0.as_raw_fd(), name_c.as_ptr(), 0o700) })?;
        let directory = self.child(name)?;
        directory.validate_private()?;
        Ok(directory)
    }

    pub fn create_file(&self, name: &OsStr) -> Result<File> {
        let file = open_at(
            self.0.as_raw_fd(),
            &component(name)?,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )?;
        platform::validate_private_file(&file)?;
        Ok(file)
    }

    pub fn read_file(&self, name: &OsStr) -> Result<File> {
        let file = open_at(self.0.as_raw_fd(), &component(name)?, libc::O_RDONLY)?;
        super::super::regular(&file)?;
        platform::validate_private_file(&file)?;
        Ok(file)
    }

    pub fn writable_file(&self, name: &OsStr) -> Result<File> {
        match self.create_file(name) {
            Ok(file) => Ok(file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let file = open_at(self.0.as_raw_fd(), &component(name)?, libc::O_RDWR)?;
                super::super::regular(&file)?;
                platform::validate_private_file(&file)?;
                Ok(file)
            }
            Err(error) => Err(error),
        }
    }

    pub fn entries(&self) -> Result<Entries> {
        let file = open_at(self.0.as_raw_fd(), c".", libc::O_RDONLY | libc::O_DIRECTORY)?;
        let pointer = unsafe { libc::fdopendir(file.as_raw_fd()) };
        if pointer.is_null() {
            return Err(io::Error::last_os_error());
        }
        // fdopendir now owns this descriptor; closedir releases it even after an early return.
        std::mem::forget(file);
        Ok(Entries(pointer))
    }

    pub fn remove_file(&self, name: &OsStr) -> Result<()> {
        let name = component(name)?;
        check(unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) })
    }

    pub fn remove_tree(&self, name: &OsStr) -> Result<()> {
        match self.child(name) {
            Ok(child) => {
                for member in child.entries()? {
                    child.remove_tree(&member?)?;
                }
                let name = component(name)?;
                check(unsafe {
                    libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                })
            }
            Err(error) if matches!(error.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) => {
                self.remove_file(name)
            }
            Err(error) => Err(error),
        }
    }

    pub fn rename_noreplace(&self, source: &OsStr, destination: &Self, name: &OsStr) -> Result<()> {
        rename(
            (self.0.as_raw_fd(), source),
            (destination.0.as_raw_fd(), name),
            true,
        )?;
        destination.0.sync_all()
    }

    pub fn rename_replace(&self, source: &OsStr, destination: &OsStr) -> Result<()> {
        rename(
            (self.0.as_raw_fd(), source),
            (self.0.as_raw_fd(), destination),
            false,
        )?;
        self.0.sync_all()
    }
}

fn rename(source: (RawFd, &OsStr), destination: (RawFd, &OsStr), exclusive: bool) -> Result<()> {
    let source_name = component(source.1)?;
    let destination_name = component(destination.1)?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            source.0,
            source_name.as_ptr(),
            destination.0,
            destination_name.as_ptr(),
            if exclusive { libc::RENAME_NOREPLACE } else { 0 },
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            source.0,
            source_name.as_ptr(),
            destination.0,
            destination_name.as_ptr(),
            if exclusive { libc::RENAME_EXCL } else { 0 },
        )
    };
    check(result)
}

pub(crate) struct Entries(*mut libc::DIR);
impl Iterator for Entries {
    type Item = Result<OsString>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            #[cfg(target_os = "linux")]
            let errno = unsafe { libc::__errno_location() };
            #[cfg(target_os = "macos")]
            let errno = unsafe { libc::__error() };
            // This DIR is owned by this iterator, never shared with another reader.
            unsafe {
                *errno = 0;
            }
            let entry = unsafe { libc::readdir(self.0) };
            if entry.is_null() {
                return (unsafe { *errno } != 0).then(|| Err(io::Error::last_os_error()));
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                return Some(Ok(OsString::from_vec(name.to_vec())));
            }
        }
    }
}
impl Drop for Entries {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}
