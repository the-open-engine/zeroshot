//! Target-owned file storage, separate from worker artifacts and workspace recovery.
//!
//! Hosts inject authenticated scope/member authorization. The disk adapter publishes immutable
//! trees through one atomic catalog; pins and managed publications keep selections alive. All
//! synchronous I/O belongs on the caller's blocking pool. No file operation falls back to a checkout.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use openengine_cluster_protocol::{
    FileAddress, FileEntry, FilePath, FileScope, FileSelection, IdempotencyKey, RunId,
};
use openengine_cluster_server::identity::ConnectionIdentity;
use serde::{Deserialize, Serialize};

mod disk;
mod paths;
pub mod publication;
mod transfer;
mod workspace;
pub use workspace::workspace_path;
pub use disk::DiskFileStore;
pub use transfer::{pack, unpack, validate_transfer};

pub type Result<T> = io::Result<T>;
pub(crate) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub(crate) fn denied() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "file access denied")
}

/// Target identity is supplied by the selected host, never derived from a path or fallback.
pub struct FileAccess<'a> {
    pub target: &'a str,
    pub identity: &'a ConnectionIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileAction {
    List,
    Read,
    Replace,
    Remove,
    Pin,
    Publish,
}

pub struct FileAuthorization<'a> {
    pub access: &'a FileAccess<'a>,
    pub action: FileAction,
    pub selection: &'a FileSelection,
}

/// Resolve an authorized storage partition on *every* directory/member lookup. Returning a
/// partition does not grant other members; the disk adapter asks again for each visited member.
/// Organization membership, per-run access and write policy remain the embedding host's authority.
pub trait FileAuthorizer: Send + Sync {
    fn authorize(&self, request: FileAuthorization<'_>) -> Result<String>;
    /// Hosts apply request-read authority to request outputs and run-read authority to final
    /// outputs. The catalog retains this context even after a source name is removed.
    fn authorize_publication(
        &self,
        request: FileAuthorization<'_>,
        _owner: &FilePublicationOwner,
    ) -> Result<()> {
        self.authorize(request).map(|_| ())
    }
}

/// Explicit single-account trust for local and unauthenticated direct targets only.
pub struct SingleAccountFiles {
    pub principal: String,
    pub tenant: String,
}
impl FileAuthorizer for SingleAccountFiles {
    fn authorize(&self, request: FileAuthorization<'_>) -> Result<String> {
        if request.access.identity.principal().as_str() != self.principal
            || request.access.identity.tenant().as_str() != self.tenant
        {
            return Err(denied());
        }
        Ok(match &request.selection.address.scope {
            FileScope::User => format!("user:{}", self.principal),
            FileScope::Org => format!("org:{}", self.tenant),
            FileScope::Run { run_id } => format!("run:{}", run_id.as_str()),
        })
    }
}

/// Private host selection. Never serialize this into a graph receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FilePin {
    pub run_id: RunId,
    pub operation: IdempotencyKey,
}

pub struct FilePinRequest {
    pub pin: FilePin,
    pub source: FileSelection,
}

/// Request and final output ownership are independent of staged run inputs and library names.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilePublicationOwner {
    Request {
        run_id: RunId,
        request: IdempotencyKey,
    },
    Final {
        run_id: RunId,
    },
}
impl FilePublicationOwner {
    pub fn run_id(&self) -> &RunId {
        match self {
            Self::Request { run_id, .. } | Self::Final { run_id } => run_id,
        }
    }
}

pub struct FilePublishRequest {
    pub owner: FilePublicationOwner,
    pub operation: IdempotencyKey,
    pub name: FilePath,
    pub pin: FilePin,
}

/// Managed receipts address retained target bytes. A live local reference is a host-only view
/// and must never be inserted in the runs namespace or a durable graph receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostFilePublication {
    Managed { address: FileAddress },
    LiveLocal { root: PathBuf, member: FilePath },
}

/// Host-selected retention boundary and work whose publications must remain available.
pub struct FileRetention<'a> {
    /// Frozen catalog identities selected before reading run liveness.
    pub candidates: &'a std::collections::BTreeSet<String>,
    pub now: openengine_cluster_protocol::UnixTimestampMillis,
    pub all: bool,
    pub protected: &'a std::collections::BTreeSet<RunId>,
}

/// Additive host port. Existing ArtifactRef and HostedWorkspaceStorage keep their original roles.
/// Operations are scoped to the authenticated target/identity. Stable operation keys make recovery
/// retries select the same bytes, or fail on conflicting reuse. Release is persistent and idempotent.
pub trait TargetFileHost: Send + Sync {
    fn list(
        &self,
        access: &FileAccess<'_>,
        scope: FileScope,
        prefix: &FilePath,
    ) -> Result<Vec<FileAddress>>;
    fn list_members(
        &self,
        access: &FileAccess<'_>,
        selection: &FileSelection,
    ) -> Result<Vec<FileEntry>>;
    fn replace(
        &self,
        access: &FileAccess<'_>,
        request: &FileReplaceRequest,
        input: &mut dyn Read,
    ) -> Result<()>;
    fn download(
        &self,
        access: &FileAccess<'_>,
        selection: &FileSelection,
        output: &mut dyn Write,
    ) -> Result<()>;
    fn remove(&self, access: &FileAccess<'_>, address: &FileAddress) -> Result<bool>;
    fn pin(&self, access: &FileAccess<'_>, request: &FilePinRequest) -> Result<FilePin>;
    fn stage(
        &self,
        access: &FileAccess<'_>,
        pin: &FilePin,
        input: &mut dyn Read,
    ) -> Result<FilePin>;
    fn read_pin(
        &self,
        access: &FileAccess<'_>,
        pin: &FilePin,
        output: &mut dyn Write,
    ) -> Result<()>;
    fn materialize(&self, access: &FileAccess<'_>, pin: &FilePin, destination: &Path)
    -> Result<()>;
    fn publish(
        &self,
        access: &FileAccess<'_>,
        request: &FilePublishRequest,
    ) -> Result<HostFilePublication>;
    fn release_publication(&self, access: &FileAccess<'_>, address: &FileAddress) -> Result<bool>;
    fn release(&self, access: &FileAccess<'_>, pin: &FilePin) -> Result<()>;
    /// Freeze eligible immutable identities before consulting the host's changing run state.
    fn select_publications(
        &self,
        _access: &FileAccess<'_>,
        _now: openengine_cluster_protocol::UnixTimestampMillis,
        _all: bool,
    ) -> Result<std::collections::BTreeSet<String>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "publication cleanup is unavailable",
        ))
    }
    fn cleanup_publications(
        &self,
        _access: &FileAccess<'_>,
        _retention: &FileRetention<'_>,
    ) -> Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "publication cleanup is unavailable",
        ))
    }
    fn reclaim(&self) -> Result<usize>;
}

pub struct FileReplaceRequest {
    pub address: FileAddress,
    pub operation: IdempotencyKey,
}

#[cfg(test)]
mod tests;

/// Private streaming scratch with a lease; restart cleanup skips transfers still owned by another
/// process. Paths are local host configuration, never accepted from a file-service request.
pub fn new_transfer(directory: &Path) -> Result<FileTransfer> {
    use fs2::FileExt;
    paths::private_root(directory)?;
    let directory = paths::Directory::open(directory)?;
    directory.validate_private()?;
    let namespace = directory.writable_file(std::ffi::OsStr::new("transfers.lock"))?;
    namespace.lock_exclusive()?;
    let temporary = paths::TemporaryFile::named(
        &directory,
        format!("transfer-{}", uuid::Uuid::now_v7()).into(),
    )?;
    temporary.file.lock_exclusive()?;
    Ok(FileTransfer(temporary))
}

/// A leased transfer whose cleanup remains anchored to its original private directory.
pub struct FileTransfer(paths::TemporaryFile);
impl FileTransfer {
    pub fn as_file(&self) -> &std::fs::File {
        &self.0.file
    }
    pub fn as_file_mut(&mut self) -> &mut std::fs::File {
        &mut self.0.file
    }
}
impl Read for FileTransfer {
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize> {
        self.0.file.read(bytes)
    }
}
impl Write for FileTransfer {
    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        self.0.file.write(bytes)
    }
    fn flush(&mut self) -> Result<()> {
        self.0.file.flush()
    }
}
impl std::io::Seek for FileTransfer {
    fn seek(&mut self, position: std::io::SeekFrom) -> Result<u64> {
        self.0.file.seek(position)
    }
}

pub fn reclaim_transfers(directory: &Path) -> Result<usize> {
    use fs2::FileExt;
    paths::private_root(directory)?;
    let directory = paths::Directory::open(directory)?;
    directory.validate_private()?;
    let namespace = directory.writable_file(std::ffi::OsStr::new("transfers.lock"))?;
    namespace.lock_exclusive()?;
    let mut count = 0;
    for entry in directory.entries()? {
        let name = entry?;
        if !name.to_string_lossy().starts_with("transfer-") {
            continue;
        }
        let file = match directory.read_file(&name) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if file.try_lock_exclusive().is_ok() {
            match directory.remove_file(&name) {
                Ok(()) => count += 1,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(count)
}
