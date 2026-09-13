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
    pub(crate) isolated_workspace: bool,
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
            isolated_workspace: specification.verifier_copy,
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
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
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
    let candidate = fs::canonicalize(&specification.candidate)?;
    let runtime_root = fs::canonicalize(
        specification
            .root
            .parent()
            .ok_or_else(|| io::Error::other("missing runtime root"))?,
    )?;
    if candidate.starts_with(&runtime_root) || runtime_root.starts_with(&candidate) {
        return Err(io::Error::other("candidate and runtime roots overlap"));
    }
    copy_entry(&candidate, workspace, specification)
}

#[cfg(not(target_os = "linux"))]
fn copy_candidate(_specification: &ExecutionFilesystemSpec, _workspace: &Path) -> io::Result<()> {
    Err(io::Error::other("hosted verifier copies require Linux"))
}

#[cfg(target_os = "linux")]
fn copy_entry(
    source: &Path,
    destination: &Path,
    specification: &ExecutionFilesystemSpec,
) -> io::Result<()> {
    check_cancelled(&specification.cancellation)?;
    let metadata = fs::symlink_metadata(source)?;
    copy_contents(source, destination, &metadata, specification)?;
    set_owner(destination, specification.identity)?;
    preserve_times(destination, &metadata)
}

#[cfg(target_os = "linux")]
fn copy_contents(
    source: &Path,
    destination: &Path,
    metadata: &fs::Metadata,
    specification: &ExecutionFilesystemSpec,
) -> io::Result<()> {
    if metadata.file_type().is_symlink() {
        return std::os::unix::fs::symlink(fs::read_link(source)?, destination);
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
    source: &Path,
    destination: &Path,
    metadata: &fs::Metadata,
    specification: &ExecutionFilesystemSpec,
) -> io::Result<()> {
    create_private_directory(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        copy_entry(
            &entry.path(),
            &destination.join(entry.file_name()),
            specification,
        )?;
    }
    fs::set_permissions(destination, metadata.permissions())
}

#[cfg(target_os = "linux")]
fn copy_file(
    source: &Path,
    destination: &Path,
    cancellation: &DriverCancellation,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let source = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(source)?;
    if !source.metadata()?.is_file() {
        return Err(io::Error::other("candidate file changed type"));
    }
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
