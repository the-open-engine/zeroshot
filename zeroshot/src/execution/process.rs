mod platform;
#[cfg(unix)]
mod platform_unix;
#[cfg(windows)]
mod platform_windows;
mod session;
mod session_io;
mod session_runtime;
mod spawn_recovery;
mod tail_buffer;

#[path = "process/diagnostic.rs"]
mod diagnostic;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

use platform::ProcessContainment;
pub(super) use diagnostic::io_error_detail;
pub use session::{
    MAX_PROCESS_FRAME_BYTES, MAX_PROCESS_FRAMING_OVERHEAD_BYTES, MAX_PROCESS_MESSAGE_BYTES,
    PROCESS_STDIN_CAPACITY, PROCESS_STDOUT_CAPACITY, ProcessFrame, ProcessOutputChunk,
    ProcessSession, ProcessSessionCommand, ProcessSessionOutput,
};
pub(crate) use session::ProcessStdout;

/// An absent deadline waits only for the caller's completion or cancellation branch.
pub(crate) async fn wait_for_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending::<()>().await,
    }
}

pub const MAX_PROCESS_DIAGNOSTIC_BYTES: usize = 64 * 1024;
/// Last-resort allocation guards for already-constructed provider commands.
///
/// Provider-owned schemas and admitted environments have substantially smaller domain bounds.
/// These limits deliberately sit far above those bounds so this generic process seam cannot
/// reject an otherwise valid provider invocation before the operating system applies its own
/// platform-specific launch limits.
pub const MAX_PROCESS_ARGV_ITEMS: usize = 16 * 1024;
pub const MAX_PROCESS_ARGV_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PROCESS_ENV_ITEMS: usize = 16 * 1024;
pub const MAX_PROCESS_ENV_BYTES: usize = 16 * 1024 * 1024;
pub const HOSTED_WORKER_UID: u32 = 10_002;
pub const HOSTED_WORKER_GID: u32 = 10_002;
pub(super) const PROCESS_RELEASE_WAIT_TIMEOUT: Duration = Duration::from_secs(1);
pub(super) const PROCESS_TREE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const PROCESS_FORCED_IO_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
pub(crate) const CONTAINED_PROCESS_CLEANUP_BUDGET: Duration = Duration::from_secs(
    PROCESS_RELEASE_WAIT_TIMEOUT.as_secs()
        + PROCESS_TREE_CLEANUP_TIMEOUT.as_secs()
        + PROCESS_FORCED_IO_DRAIN_TIMEOUT.as_secs(),
);

pub(crate) fn write_new_file(path: &Path, bytes: &[u8], unix_mode: u32) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(unix_mode);
    }
    #[cfg(not(unix))]
    let _ = unix_mode;
    let mut file = options.open(path)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

/// Linux identity allocation for one contained provider process domain.
///
/// Writers share the workspace owner UID and use session-specific supplementary groups for
/// process cleanup. Verifiers retain distinct UIDs. A production host reserves disjoint UID and
/// supplementary-group ranges for each active run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostedProcessPool {
    writer_uid: u32,
    writer_gid: u32,
    session_identity_base: u32,
    verifier_gid: u32,
}

/// Stable containment and runtime-home scope for one provider session.
///
/// Node-instance scopes survive authored loop revisits. Execution scopes are deliberately fresh.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedProcessScope {
    Writer,
    WriterNodeInstance(u64),
    WriterExecution(u64),
    VerifierNodeInstance(u64),
    VerifierExecution(u64),
}

impl HostedProcessScope {
    #[must_use]
    pub fn private_home(self, root: &Path) -> PathBuf {
        let leaf = match self {
            Self::Writer => "writer".to_owned(),
            Self::WriterNodeInstance(identity) => format!("writer-node-instance-{identity}"),
            Self::WriterExecution(identity) => format!("writer-execution-{identity}"),
            Self::VerifierNodeInstance(identity) => {
                format!("verifier-node-instance-{identity}")
            }
            Self::VerifierExecution(identity) => format!("verifier-execution-{identity}"),
        };
        root.join(leaf)
    }

    fn session_identity(self) -> Option<(u64, u32)> {
        match self {
            Self::Writer => None,
            Self::WriterNodeInstance(identity) => Some((identity, 2)),
            Self::WriterExecution(identity) => Some((identity, 3)),
            Self::VerifierNodeInstance(identity) => Some((identity, 0)),
            Self::VerifierExecution(identity) => Some((identity, 1)),
        }
    }

    fn validate(self) -> Result<(), ProcessRunnerError> {
        let identity = match self {
            Self::Writer => None,
            Self::WriterNodeInstance(identity)
            | Self::WriterExecution(identity)
            | Self::VerifierNodeInstance(identity)
            | Self::VerifierExecution(identity) => Some(identity),
        };
        if identity == Some(0) {
            return Err(ProcessRunnerError::InvalidCommand(
                "provider process identity must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HostedProcessIdentity {
    runner: LocalProcessRunner,
    uid: u32,
    gid: u32,
    scope: HostedProcessScope,
}

impl HostedProcessIdentity {
    #[must_use]
    pub const fn runner(self) -> LocalProcessRunner {
        self.runner
    }

    #[must_use]
    pub const fn uid(self) -> u32 {
        self.uid
    }

    #[must_use]
    pub const fn gid(self) -> u32 {
        self.gid
    }

    /// Creates or reclaims the provider-private leaf under a supervisor-owned runtime root.
    ///
    /// The root must already exist and be traversable by the configured provider identity. It must
    /// not be writable by provider processes; only the generated leaf is handed to the child.
    pub fn prepare_private_home(self, root: &Path) -> Result<PathBuf, ProcessRunnerError> {
        let home = self.scope.private_home(root);
        prepare_private_directory(&home, Some((self.uid, self.gid)))?;
        Ok(home)
    }
}

impl HostedProcessPool {
    pub(crate) const fn hosted_default() -> Self {
        Self {
            writer_uid: HOSTED_WORKER_UID,
            writer_gid: HOSTED_WORKER_GID,
            session_identity_base: 20_000,
            verifier_gid: 20_000,
        }
    }

    pub fn new(
        writer_uid: u32,
        writer_gid: u32,
        session_identity_base: u32,
        verifier_gid: u32,
    ) -> Result<Self, ProcessRunnerError> {
        if writer_uid == 0
            || writer_gid == 0
            || session_identity_base == 0
            || verifier_gid == 0
            || session_identity_base == u32::MAX
            || writer_uid >= session_identity_base
        {
            return Err(ProcessRunnerError::InvalidCommand(
                "hosted provider identities are invalid".to_owned(),
            ));
        }
        Ok(Self {
            writer_uid,
            writer_gid,
            session_identity_base,
            verifier_gid,
        })
    }

    pub fn writer(self) -> Result<LocalProcessRunner, ProcessRunnerError> {
        self.identity(HostedProcessScope::Writer)
            .map(HostedProcessIdentity::runner)
    }

    pub fn verifier(self, execution: u64) -> Result<LocalProcessRunner, ProcessRunnerError> {
        self.identity(HostedProcessScope::VerifierExecution(execution))
            .map(HostedProcessIdentity::runner)
    }

    /// Derives one disjoint active-run pool from this host pool.
    ///
    /// The host pool's writer identity remains reserved for serialized source resolution. Active
    /// runs start at its session base and reserve one workspace owner plus both writer group and
    /// verifier UID session variants for every admitted execution identity.
    pub(crate) fn active_run_slot(
        self,
        slot: u32,
        maximum_identity: u64,
    ) -> Result<Self, ProcessRunnerError> {
        let width = maximum_identity
            .checked_mul(4)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(identity_range_exhausted)?;
        let session_span = maximum_identity
            .checked_mul(4)
            .and_then(|value| value.checked_sub(1))
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(identity_range_exhausted)?;
        let offset = u64::from(slot)
            .checked_mul(width)
            .ok_or_else(identity_range_exhausted)?;
        let writer_uid = u64::from(self.session_identity_base)
            .checked_add(offset)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(identity_range_exhausted)?;
        let session_identity_base = writer_uid
            .checked_add(1)
            .ok_or_else(identity_range_exhausted)?;
        let highest_uid = session_identity_base
            .checked_add(session_span)
            .ok_or_else(identity_range_exhausted)?;
        if highest_uid == u32::MAX {
            return Err(identity_range_exhausted());
        }
        Self::new(
            writer_uid,
            self.writer_gid,
            session_identity_base,
            self.verifier_gid,
        )
    }

    pub fn identity(
        self,
        scope: HostedProcessScope,
    ) -> Result<HostedProcessIdentity, ProcessRunnerError> {
        scope.validate()?;
        let (uid, gid, group) = match scope.session_identity() {
            None => (self.writer_uid, self.writer_gid, None),
            Some((identity, discriminator)) => {
                let index = identity.checked_sub(1).ok_or_else(|| {
                    ProcessRunnerError::InvalidCommand(
                        "provider process identity must be greater than zero".to_owned(),
                    )
                })?;
                let offset = index
                    .checked_mul(4)
                    .and_then(|value| value.checked_add(u64::from(discriminator)))
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| {
                        ProcessRunnerError::InvalidCommand(
                            "provider identity range is exhausted".to_owned(),
                        )
                    })?;
                let identity = self
                    .session_identity_base
                    .checked_add(offset)
                    .ok_or_else(|| {
                        ProcessRunnerError::InvalidCommand(
                            "provider identity range is exhausted".to_owned(),
                        )
                    })?;
                if discriminator >= 2 {
                    (self.writer_uid, self.writer_gid, Some(identity))
                } else {
                    (identity, self.verifier_gid, None)
                }
            }
        };
        let runner = LocalProcessRunner::hosted_identity(uid, gid, group)?;
        Ok(HostedProcessIdentity {
            runner,
            uid,
            gid,
            scope,
        })
    }
}

fn identity_range_exhausted() -> ProcessRunnerError {
    ProcessRunnerError::InvalidCommand("hosted process identity range is exhausted".to_owned())
}

pub fn prepare_local_private_home(
    root: &Path,
    scope: HostedProcessScope,
) -> Result<PathBuf, ProcessRunnerError> {
    scope.validate()?;
    let home = scope.private_home(root);
    prepare_private_directory(&home, None)?;
    Ok(home)
}

fn prepare_private_directory(
    path: &Path,
    owner: Option<(u32, u32)>,
) -> Result<(), ProcessRunnerError> {
    let created = create_private_directory(path)?;
    let result = validate_private_directory(path)
        .and_then(|()| set_private_directory_mode(path))
        .and_then(|()| set_private_directory_owner(path, owner));
    if created && result.is_err() {
        // Only remove the empty directory created by this attempt; retained session homes survive.
        let _ = std::fs::remove_dir(path);
    }
    result
}

fn create_private_directory(path: &Path) -> Result<bool, ProcessRunnerError> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(ProcessRunnerError::Launch(io_error_detail(
            "provider private home create failed",
            &error,
        ))),
    }
}

fn validate_private_directory(path: &Path) -> Result<(), ProcessRunnerError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        ProcessRunnerError::Launch(io_error_detail(
            "provider private home inspection failed",
            &error,
        ))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ProcessRunnerError::InvalidCommand(
            "provider private home is not a directory".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_mode(path: &Path) -> Result<(), ProcessRunnerError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|error| {
        ProcessRunnerError::Launch(io_error_detail(
            "provider private home chmod failed",
            &error,
        ))
    })
}

#[cfg(not(unix))]
fn set_private_directory_mode(_path: &Path) -> Result<(), ProcessRunnerError> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_private_directory_owner(
    path: &Path,
    owner: Option<(u32, u32)>,
) -> Result<(), ProcessRunnerError> {
    use std::os::unix::ffi::OsStrExt;

    if let Some((uid, gid)) = owner {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            ProcessRunnerError::InvalidCommand("provider private home path is invalid".to_owned())
        })?;
        // SAFETY: the path is a live NUL-free C string and the caller supplied fixed numeric IDs.
        if unsafe { libc::chown(path.as_ptr(), uid, gid) } != 0 {
            return Err(ProcessRunnerError::Launch(io_error_detail(
                "provider private home chown failed",
                &std::io::Error::last_os_error(),
            )));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn set_private_directory_owner(
    _path: &Path,
    owner: Option<(u32, u32)>,
) -> Result<(), ProcessRunnerError> {
    let _ = owner;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessLaunchEvidence {
    DefinitelyNotStarted,
    MayHaveStarted,
}

#[derive(Debug, Error)]
pub enum ProcessRunnerError {
    #[error("invalid process command: {0}")]
    InvalidCommand(String),
    #[error("process launch failed before start: {0}")]
    Launch(String),
    #[error("process I/O failed after launch: {0}")]
    Io(String),
}

impl ProcessRunnerError {
    #[must_use]
    pub const fn launch_evidence(&self) -> ProcessLaunchEvidence {
        match self {
            Self::InvalidCommand(_) | Self::Launch(_) => {
                ProcessLaunchEvidence::DefinitelyNotStarted
            }
            Self::Io(_) => ProcessLaunchEvidence::MayHaveStarted,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LocalProcessRunner {
    containment: ProcessContainment,
}

impl Default for LocalProcessRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalProcessRunner {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            containment: ProcessContainment::ProcessGroup,
        }
    }

    pub fn hosted_worker() -> Result<Self, ProcessRunnerError> {
        Self::hosted_worker_identity(HOSTED_WORKER_UID, HOSTED_WORKER_GID)
    }

    pub fn hosted_worker_identity(uid: u32, gid: u32) -> Result<Self, ProcessRunnerError> {
        Self::hosted_identity(uid, gid, None)
    }

    fn hosted_identity(uid: u32, gid: u32, group: Option<u32>) -> Result<Self, ProcessRunnerError> {
        #[cfg(target_os = "linux")]
        {
            if uid == 0 || gid == 0 || group.is_some_and(|group| group == 0 || group == u32::MAX) {
                return Err(ProcessRunnerError::InvalidCommand(
                    "hosted worker identity must be unprivileged".to_owned(),
                ));
            }
            let containment = match group {
                Some(group) => ProcessContainment::WorkerGroup { uid, gid, group },
                None => ProcessContainment::WorkerUid { uid, gid },
            };
            Ok(Self { containment })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (uid, gid, group);
            Err(ProcessRunnerError::Launch(
                "hosted worker containment requires Linux".to_owned(),
            ))
        }
    }
}

pub use platform::ProcessCleanupEvidence;

#[cfg(test)]
#[path = "process/tests.rs"]
mod tests;
