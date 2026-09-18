//! Disposable build workspaces and session-private homes for provider processes.

use std::fs;
use std::io;
#[cfg(target_os = "linux")]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Mutex;

use crate::execution::driver::DriverCancellation;
use crate::execution::process::{HostedProcessIdentity, LocalProcessRunner, ProcessRunnerError};
use crate::native_v2_runner::{DriverControl, DriverInvocation, NodeRole};

use super::{ProviderProcessRunners, ProviderSessionCore, process_scope};

#[derive(Clone, Copy)]
pub(crate) struct ProviderFilesystemConfig<'a> {
    pub(crate) runners: ProviderProcessRunners,
    pub(crate) root: &'a Path,
    pub(crate) workspace: &'a Path,
}

pub(crate) struct ProviderExecution<'a> {
    config: ProviderFilesystemConfig<'a>,
    invocation: &'a DriverInvocation,
    session: &'a ProviderSessionCore,
    files: Mutex<Option<Arc<ProviderExecutionFiles>>>,
}

impl<'a> ProviderExecution<'a> {
    pub(crate) fn new(
        config: ProviderFilesystemConfig<'a>,
        invocation: &'a DriverInvocation,
        session: &'a ProviderSessionCore,
    ) -> Self {
        Self {
            config,
            invocation,
            session,
            files: Mutex::new(None),
        }
    }

    pub(crate) async fn prepare(
        &self,
        control: &DriverControl,
    ) -> Result<Arc<ProviderExecutionFiles>, ProcessRunnerError> {
        let mut files = self.files.lock().await;
        if let Some(files) = files.as_ref() {
            files.ensure_idle()?;
            return Ok(files.clone());
        }
        let specification = self.specification(control).await?;
        let prepared =
            tokio::task::spawn_blocking(move || ProviderExecutionFiles::prepare(specification))
                .await
                .map_err(|_| {
                    ProcessRunnerError::Launch(
                        "provider filesystem preparation task failed".to_owned(),
                    )
                })??;
        let prepared = Arc::new(prepared);
        *files = Some(prepared.clone());
        Ok(prepared)
    }

    async fn specification(
        &self,
        control: &DriverControl,
    ) -> Result<ExecutionFilesystemSpec, ProcessRunnerError> {
        let scope = process_scope(self.invocation).map_err(|_| {
            ProcessRunnerError::InvalidCommand("provider filesystem scope is invalid".to_owned())
        })?;
        let (runner, home) = self.config.runners.turn_process(self.config.root, scope)?;
        let home = self.session.retain_home(home).await?;
        let identity = match self.config.runners {
            ProviderProcessRunners::Hosted(pool) => Some(pool.identity(scope)?),
            ProviderProcessRunners::Local => None,
        };
        Ok(ExecutionFilesystemSpec {
            runner,
            home,
            root: self.config.root.join(format!(
                "execution-{}",
                self.invocation.node.reference.execution.get()
            )),
            candidate: self.config.workspace.to_owned(),
            verifier_copy: identity.is_some() && self.invocation.role == NodeRole::Verifier,
            identity,
            cancellation: control.cancellation(),
        })
    }
}

pub(super) struct PrivateDirectory {
    path: PathBuf,
    processes: AtomicUsize,
}

impl PrivateDirectory {
    pub(super) fn retained(path: PathBuf) -> Self {
        Self {
            path,
            processes: AtomicUsize::new(0),
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    fn create(path: PathBuf) -> io::Result<Self> {
        create_private_directory(&path)?;
        Ok(Self::retained(path))
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        // An unproven process exit must leave files intact for whole-run cleanup.
        if self.processes.load(Ordering::Acquire) != 0 {
            return;
        }
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| metadata.is_dir()) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct ExecutionFilesystemSpec {
    runner: LocalProcessRunner,
    home: Arc<PrivateDirectory>,
    root: PathBuf,
    candidate: PathBuf,
    verifier_copy: bool,
    identity: Option<HostedProcessIdentity>,
    cancellation: DriverCancellation,
}

pub(crate) struct ProviderExecutionFiles {
    pub(crate) runner: LocalProcessRunner,
    pub(crate) workspace: PathBuf,
    scratch: PathBuf,
    home: Arc<PrivateDirectory>,
    root: PrivateDirectory,
}

impl ProviderExecutionFiles {
    fn prepare(specification: ExecutionFilesystemSpec) -> Result<Self, ProcessRunnerError> {
        Self::prepare_io(specification).map_err(|error| {
            ProcessRunnerError::Launch(format!("provider filesystem preparation failed: {error}"))
        })
    }

    fn prepare_io(specification: ExecutionFilesystemSpec) -> io::Result<Self> {
        check_cancelled(&specification.cancellation)?;
        let root = PrivateDirectory::create(specification.root.clone())?;
        let scratch = root.path.join("tmp");
        create_private_directory(&scratch)?;
        set_owner(&scratch, specification.identity)?;
        let workspace = if specification.verifier_copy {
            let workspace = root.path.join("workspace");
            copy_candidate(&specification, &workspace)?;
            workspace
        } else {
            specification.candidate
        };
        set_owner(&root.path, specification.identity)?;
        Ok(Self {
            runner: specification.runner,
            workspace,
            scratch,
            home: specification.home,
            root,
        })
    }

    pub(crate) fn home(&self) -> &Path {
        self.home.path()
    }

    pub(crate) fn scratch_text(&self) -> Result<String, ProcessRunnerError> {
        self.scratch.to_str().map(str::to_owned).ok_or_else(|| {
            ProcessRunnerError::InvalidCommand(
                "provider scratch path is not valid UTF-8".to_owned(),
            )
        })
    }

    pub(crate) fn begin_process(&self) {
        self.root.processes.fetch_add(1, Ordering::AcqRel);
        self.home.processes.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn process_reaped(&self) {
        self.root.processes.fetch_sub(1, Ordering::AcqRel);
        self.home.processes.fetch_sub(1, Ordering::AcqRel);
    }

    fn ensure_idle(&self) -> Result<(), ProcessRunnerError> {
        if self.root.processes.load(Ordering::Acquire) != 0 {
            return Err(ProcessRunnerError::Launch(
                "previous provider process cleanup was not confirmed".to_owned(),
            ));
        }
        Ok(())
    }
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    crate::execution::platform::create_private_directory(path)
}

fn check_cancelled(cancellation: &DriverCancellation) -> io::Result<()> {
    if cancellation.is_cancelled() {
        return Err(io::Error::other(
            "provider filesystem preparation cancelled",
        ));
    }
    Ok(())
}

fn set_owner(path: &Path, identity: Option<HostedProcessIdentity>) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    if let Some(identity) = identity {
        std::os::unix::fs::lchown(path, Some(identity.uid()), Some(identity.gid()))?;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (path, identity);
    Ok(())
}

#[cfg(target_os = "linux")]
fn copy_candidate(specification: &ExecutionFilesystemSpec, workspace: &Path) -> io::Result<()> {
    let source = open_copy_root(&specification.candidate)?;
    let candidate = fs::read_link(copy_source_path(&source))?;
    let runtime_root = fs::canonicalize(
        specification
            .root
            .parent()
            .ok_or_else(|| io::Error::other("missing runtime root"))?,
    )?;
    if candidate.starts_with(&runtime_root) || runtime_root.starts_with(&candidate) {
        return Err(io::Error::other("candidate and runtime roots overlap"));
    }
    copy_entry(source, workspace, specification)
}

#[cfg(not(target_os = "linux"))]
fn copy_candidate(_specification: &ExecutionFilesystemSpec, _workspace: &Path) -> io::Result<()> {
    Err(io::Error::other("hosted verifier copies require Linux"))
}

// Hosted filesystem preparation canonicalizes paths before writers start. Pin every component
// without resolving fresh symlinks: O_NOFOLLOW on an absolute open protects only its leaf.
#[cfg(target_os = "linux")]
fn open_copy_root(path: &Path) -> io::Result<fs::File> {
    use std::os::fd::AsRawFd;
    use std::path::Component;

    let path = std::path::absolute(path)?;
    let mut source = open_copy_directory(libc::AT_FDCWD, Path::new("/"))?;
    let mut parents = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                if let Some(parent) = parents.pop() {
                    source = parent;
                }
            }
            Component::Normal(name) => {
                let child = open_copy_directory(source.as_raw_fd(), Path::new(name))?;
                parents.push(source);
                source = child;
            }
            Component::Prefix(_) => return Err(io::Error::other("invalid candidate path")),
        }
    }
    Ok(source)
}

#[cfg(target_os = "linux")]
fn open_copy_directory(parent: std::os::fd::RawFd, name: &Path) -> io::Result<fs::File> {
    let source = open_copy_source(parent, name)?;
    if !source.metadata()?.is_dir() {
        return Err(io::Error::other("candidate path is not a directory"));
    }
    Ok(source)
}

// O_PATH pins an entry without following symlinks or opening FIFOs/devices for I/O.
#[cfg(target_os = "linux")]
fn open_copy_source(parent: std::os::fd::RawFd, name: &Path) -> io::Result<fs::File> {
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;

    let name = std::ffi::CString::new(name.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("invalid candidate path"))?;
    // SAFETY: name is NUL-terminated and parent is a live directory descriptor or AT_FDCWD.
    let descriptor = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new descriptor whose ownership is transferred to File exactly once.
    Ok(unsafe { fs::File::from_raw_fd(descriptor) })
}

#[cfg(target_os = "linux")]
fn copy_source_path(source: &fs::File) -> PathBuf {
    use std::os::fd::AsRawFd;

    PathBuf::from(format!("/proc/self/fd/{}", source.as_raw_fd()))
}

#[cfg(target_os = "linux")]
fn read_copy_symlink(source: &fs::File) -> io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;

    let mut buffer = [0_u8; libc::PATH_MAX as usize];
    // SAFETY: source pins a symlink, the empty name selects that inode, and buffer is writable.
    let length = unsafe {
        libc::readlinkat(
            source.as_raw_fd(),
            c"".as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    let length = usize::try_from(length).map_err(|_| io::Error::last_os_error())?;
    if length == buffer.len() {
        return Err(io::Error::other(
            "candidate symlink target exceeds its bound",
        ));
    }
    Ok(std::ffi::OsString::from_vec(buffer[..length].to_vec()).into())
}

#[cfg(target_os = "linux")]
fn copy_entry(
    source: fs::File,
    destination: &Path,
    specification: &ExecutionFilesystemSpec,
) -> io::Result<()> {
    check_cancelled(&specification.cancellation)?;
    let metadata = source.metadata()?;
    copy_contents(&source, destination, &metadata, specification)?;
    set_owner(destination, specification.identity)?;
    preserve_times(destination, &metadata)
}

#[cfg(target_os = "linux")]
fn copy_contents(
    source: &fs::File,
    destination: &Path,
    metadata: &fs::Metadata,
    specification: &ExecutionFilesystemSpec,
) -> io::Result<()> {
    if metadata.file_type().is_symlink() {
        return std::os::unix::fs::symlink(read_copy_symlink(source)?, destination);
    }
    if metadata.is_dir() {
        return copy_directory(source, destination, metadata, specification);
    }
    if !metadata.is_file() {
        return Err(io::Error::other(
            "candidate contains an unsupported filesystem entry",
        ));
    }
    copy_file(source, destination, &specification.cancellation)?;
    fs::set_permissions(destination, metadata.permissions())
}

#[cfg(target_os = "linux")]
fn copy_directory(
    source: &fs::File,
    destination: &Path,
    metadata: &fs::Metadata,
    specification: &ExecutionFilesystemSpec,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    create_private_directory(destination)?;
    for entry in fs::read_dir(copy_source_path(source))? {
        check_cancelled(&specification.cancellation)?;
        let name = entry?.file_name();
        // Resolve only this child against the pinned parent; a renamed ancestor is never followed.
        let child = open_copy_source(source.as_raw_fd(), Path::new(&name))?;
        copy_entry(child, &destination.join(name), specification)?;
    }
    fs::set_permissions(destination, metadata.permissions())
}

#[cfg(target_os = "linux")]
fn copy_file(
    source: &fs::File,
    destination: &Path,
    cancellation: &DriverCancellation,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    // The O_PATH handle pins this regular inode even if writers rename or replace its entry.
    let source = fs::File::open(copy_source_path(source))?;
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)?;
    // SAFETY: both descriptors refer to live, independently opened regular files.
    let cloned =
        unsafe { libc::ioctl(destination.as_raw_fd(), libc::FICLONE, source.as_raw_fd()) } == 0;
    if !cloned {
        io::copy(
            &mut CancellableReader {
                source,
                cancellation,
            },
            &mut destination,
        )?;
    }
    check_cancelled(cancellation)
}

#[cfg(target_os = "linux")]
struct CancellableReader<'a> {
    source: fs::File,
    cancellation: &'a DriverCancellation,
}

#[cfg(target_os = "linux")]
impl Read for CancellableReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        check_cancelled(self.cancellation)?;
        self.source.read(buffer)
    }
}

#[cfg(target_os = "linux")]
fn preserve_times(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("invalid copied path"))?;
    let times = [
        libc::timespec {
            tv_sec: metadata.atime(),
            tv_nsec: metadata.atime_nsec(),
        },
        libc::timespec {
            tv_sec: metadata.mtime(),
            tv_nsec: metadata.mtime_nsec(),
        },
    ];
    // SAFETY: the path and both timestamps remain live; symlink targets are never followed.
    if unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            path.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
