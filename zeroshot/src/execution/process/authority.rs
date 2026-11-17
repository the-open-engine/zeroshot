//! Local CLI/RPC provenance under native account permissions, not an OS sandbox.
//! Ownership is published before provider execution and retained through tree cleanup.
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use crate::execution::platform::{self, FileAccess};

mod native;
#[cfg(test)]
mod tests;
#[cfg(unix)]
mod unix;

pub(crate) const MANDATE_REQUIRED: &str = "node messages require an authorized --network";
pub(crate) const HUMAN_STEERING_ATTRIBUTE: &str = "zeroshot.localHumanSteering";
const MAX_ANCESTORS: usize = 512;
const RECORD_BYTES: usize = 16;

/// The PID comes from the current process or transport peer credentials, never a request.
pub(crate) fn require_human(pid: u32) -> io::Result<()> {
    let directory = native::registry_directory()?;
    match platform::open_directory(&directory) {
        Ok(file) => platform::validate_private_file(&file)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let caller = native::process_identity(pid)?;
    let mut current = caller;
    for _ in 0..MAX_ANCESTORS {
        if owned(&directory, current)? || owned_group(&directory, current.group)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                MANDATE_REQUIRED,
            ));
        }
        if current.parent == 0 {
            return Ok(());
        }
        let parent = parent_or_boundary(caller, current)?;
        if native::process_identity(current.pid)? != current {
            return Err(io::Error::other("local steering caller ancestry changed"));
        }
        let Some(parent) = parent else {
            return Ok(());
        };
        current = parent;
    }
    Err(io::Error::other(
        "local steering caller ancestry exceeds its bound",
    ))
}

// A departed parent can hide an owned ancestor. Only native Job exclusion can end that search.
fn parent_or_boundary(
    caller: native::ProcessIdentity,
    current: native::ProcessIdentity,
) -> io::Result<Option<native::ProcessIdentity>> {
    let error = match native::process_identity(current.parent) {
        Ok(parent) if parent.born <= current.born => return Ok(Some(parent)),
        Ok(_) => io::Error::other("local steering caller ancestry changed"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => error,
        Err(error) => return Err(error),
    };
    if native::outside_jobs(caller)? {
        Ok(None)
    } else {
        Err(error)
    }
}

fn read_record(directory: &Path, pid: u32) -> io::Result<Option<File>> {
    let file = match platform::private_file(&directory.join(pid.to_string()), FileAccess::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() != RECORD_BYTES as u64 {
        return Err(io::Error::other("incomplete local process ownership"));
    }
    Ok(Some(file))
}

fn record_born(file: &mut File, pid: u32) -> io::Result<u64> {
    let mut bytes = [0; RECORD_BYTES];
    file.read_exact(&mut bytes)?;
    if u64::from_le_bytes(bytes[..8].try_into().expect("fixed PID bytes")) != u64::from(pid) {
        return Err(io::Error::other("invalid local process ownership"));
    }
    Ok(u64::from_le_bytes(
        bytes[8..].try_into().expect("fixed creation bytes"),
    ))
}

// Zero means live or provisional ownership; leases are checked before Windows reads.
fn ownership_stamp(directory: &Path, pid: u32) -> io::Result<Option<u64>> {
    let Some(mut file) = read_record(directory, pid)? else {
        return Ok(None);
    };
    if lease_active(&file)? {
        return Ok(Some(0));
    }
    record_born(&mut file, pid).map(Some)
}

fn owned(directory: &Path, identity: native::ProcessIdentity) -> io::Result<bool> {
    Ok(ownership_stamp(directory, identity.pid)?
        .is_some_and(|born| born == 0 || born == identity.born))
}

fn owned_group(directory: &Path, group: u32) -> io::Result<bool> {
    if group == 0 {
        return Ok(false);
    }
    match ownership_stamp(directory, group)? {
        None => Ok(false),
        Some(0) => Ok(true),
        Some(born) => match native::process_identity(group) {
            Ok(leader) => Ok(leader.born == born),
            // An existing native group can retain descendants after its leader exits.
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error),
        },
    }
}

fn lease_active(file: &File) -> io::Result<bool> {
    match FileExt::try_lock_shared(file) {
        Ok(()) => {
            FileExt::unlock(file)?;
            Ok(false)
        }
        Err(error) if platform::lock_is_contended(&error) => Ok(true),
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
pub(super) struct Registration {
    directory: PathBuf,
    pending: String,
    file: File,
    publication_lock: Option<File>,
}

impl Registration {
    pub(super) fn prepare() -> io::Result<Self> {
        let directory = native::registry_directory()?;
        platform::ensure_private_directory(&directory)?;
        let publication_lock = lock_publication(&directory)?;
        let pending = format!(".pending-{}", uuid::Uuid::now_v7());
        let mut file = platform::private_file(&directory.join(&pending), FileAccess::CreateNew)?;
        #[cfg(unix)]
        {
            file = File::from(platform::retained_descriptor(&file)?);
        }
        file.write_all(&[0; RECORD_BYTES])?;
        FileExt::lock_exclusive(&file)?;
        Ok(Self {
            directory,
            pending,
            file,
            publication_lock: Some(publication_lock),
        })
    }

    pub(super) fn configure(&self, command: &mut tokio::process::Command) -> io::Result<()> {
        #[cfg(unix)]
        unix::configure(command, &self.directory, &self.pending, &self.file)?;
        #[cfg(not(unix))]
        let _ = command;
        Ok(())
    }

    pub(super) fn capture(&mut self, pid: u32, born: Option<u64>) -> io::Result<()> {
        #[cfg(windows)]
        {
            self.file.seek(SeekFrom::Start(0))?;
            self.file.write_all(&u64::from(pid).to_le_bytes())?;
            std::fs::rename(
                self.directory.join(&self.pending),
                self.directory.join(pid.to_string()),
            )?;
        }
        let born = match born {
            Some(born) => Some(born),
            None => match native::process_identity(pid) {
                Ok(identity) => Some(identity.born),
                // A short command may already have ended; its lease still denies through cleanup.
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            },
        };
        if let Some(born) = born {
            self.file.seek(SeekFrom::Start(8))?;
            self.file.write_all(&born.to_le_bytes())?;
        }
        self.publication_lock = None;
        Ok(())
    }

    #[cfg(windows)]
    pub(super) fn capture_windows(
        &mut self,
        pid: u32,
        handle: std::os::windows::io::RawHandle,
    ) -> io::Result<()> {
        self.capture(pid, Some(native::root_born(handle)?))
    }

    #[cfg(target_os = "linux")]
    pub(super) fn lease(&self) -> io::Result<File> {
        platform::retained_descriptor(&self.file).map(File::from)
    }

    /// Called only after the native containment proves that its whole tree is empty.
    pub(super) fn retire(&mut self) -> io::Result<()> {
        let _lock = if self.publication_lock.is_none() {
            Some(lock_publication(&self.directory)?)
        } else {
            None
        };
        self.file.seek(SeekFrom::Start(0))?;
        let mut pid = [0; 8];
        self.file.read_exact(&mut pid)?;
        let path = self.directory.join(u64::from_le_bytes(pid).to_string());
        let current = match platform::open_identity(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if platform::file_identity(&current)? == platform::file_identity(&self.file)? {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // No explicit unlock: the Linux supervisor may still hold the same open description.
        let _ = std::fs::remove_file(self.directory.join(&self.pending));
    }
}

fn lock_publication(directory: &Path) -> io::Result<File> {
    let file = platform::private_file(&directory.join(".publication-lock"), FileAccess::ReadWrite)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(file),
            Err(error) if platform::lock_is_contended(&error) => {
                if std::time::Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "process ownership publication is busy",
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(error) => return Err(error),
        }
    }
}
