//! Immutable local workspace copies. Callers hold the workspace's execution barrier throughout
//! capture and restore; this module does not stop providers or reconstruct scheduler state.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::execution::platform::{self, FileAccess};
use crate::native_v2_capsule::provider_process::copy_workspace_entry;

#[path = "filesystem/git.rs"]
mod git;
#[cfg(test)]
#[path = "filesystem/tests.rs"]
mod tests;

const MAX_METADATA_BYTES: u64 = 1_048_576;
const FORMAT: u32 = 1;

/// Opaque directory identity; deserialization rejects paths and noncanonical values.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct SnapshotId(String);

impl SnapshotId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn new() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }
}

impl TryFrom<String> for SnapshotId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parsed = uuid::Uuid::parse_str(&value).map_err(|_| "invalid snapshot identity")?;
        if parsed.get_version_num() != 7 || parsed.to_string() != value {
            return Err("invalid snapshot identity");
        }
        Ok(Self(value))
    }
}

impl From<SnapshotId> for String {
    fn from(value: SnapshotId) -> Self {
        value.0
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotMetadata<T> {
    format: u32,
    git: bool,
    user_metadata: T,
}

/// Publishes the complete tree and caller metadata with one directory rename.
/// The containing directory remains private to the supervisor, even when copied files are writable.
pub fn capture<T: Serialize>(
    workspace: &Path,
    snapshots_root: &Path,
    metadata: &T,
) -> io::Result<SnapshotId> {
    let workspace = existing_directory(workspace)?;
    let snapshots_root = prepare_snapshots_root(&workspace, snapshots_root)?;
    let stage = private_stage(&snapshots_root, ".capture-")?;
    let tree = stage.path().join("workspace");
    copy_workspace_entry(&workspace, &tree)?;
    publish_capture(&workspace, stage.path(), &snapshots_root, metadata)
}

fn publish_capture<T: Serialize>(
    workspace: &Path,
    stage: &Path,
    snapshots_root: &Path,
    metadata: &T,
) -> io::Result<SnapshotId> {
    let tree = stage.join("workspace");
    let manifest = SnapshotMetadata {
        format: FORMAT,
        git: git::capture(workspace, &tree)?,
        user_metadata: metadata,
    };
    write_metadata(stage, &manifest)?;
    sync_tree(&tree)?;
    sync_directory(stage)?;
    let id = SnapshotId::new();
    fs::rename(stage, snapshots_root.join(id.as_str()))?;
    sync_directory(snapshots_root)?;
    Ok(id)
}

/// Returns only caller-owned metadata from a fully published snapshot.
#[cfg(test)]
pub fn metadata<T: DeserializeOwned>(snapshots_root: &Path, id: &SnapshotId) -> io::Result<T> {
    let snapshot = snapshot_directory(snapshots_root, id)?;
    Ok(read_metadata::<T>(&snapshot)?.user_metadata)
}

/// Stages an independent copy before replacing the selected workspace's children.
/// An interrupted restore is retryable from the same immutable source; callers must not dispatch
/// execution until this function succeeds. Existing Git administrative identities stay in place.
pub fn restore(snapshots_root: &Path, id: &SnapshotId, workspace: &Path) -> io::Result<()> {
    let snapshot = snapshot_directory(snapshots_root, id)?;
    let manifest = read_metadata::<serde_json::Value>(&snapshot)?;
    let workspace = restore_destination(workspace)?;
    ensure_disjoint(&workspace, &fs::canonicalize(snapshots_root)?)?;
    let target_git = git::restore_target(&workspace, manifest.git)?;
    let parent = workspace
        .parent()
        .ok_or_else(|| invalid("workspace has no parent"))?;
    let stage = private_stage(parent, ".restore-")?;
    let tree = stage.path().join("workspace");
    copy_workspace_entry(&snapshot.join("workspace"), &tree)?;
    apply_restore(&tree, &workspace, target_git)
}

fn apply_restore(
    tree: &Path,
    workspace: &Path,
    target_git: Option<git::GitLayout>,
) -> io::Result<()> {
    let permissions = fs::metadata(tree)?.permissions();
    if !workspace.exists() {
        platform::create_private_directory(workspace)?;
    }
    replace_children(tree, workspace, target_git.is_some())?;
    if let Some(layout) = target_git {
        git::restore_existing(&tree.join(".git"), &layout)?;
    }
    fs::set_permissions(workspace, permissions)?;
    sync_directory(workspace)
}

fn replace_children(tree: &Path, workspace: &Path, preserve_git: bool) -> io::Result<()> {
    clear_workspace(workspace, preserve_git)?;
    install_children(tree, workspace, preserve_git)
}

fn clear_workspace(workspace: &Path, preserve_git: bool) -> io::Result<()> {
    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        if !preserve_git || entry.file_name() != ".git" {
            remove_entry(&entry.path())?;
        }
    }
    Ok(())
}

fn install_children(tree: &Path, workspace: &Path, preserve_git: bool) -> io::Result<()> {
    for entry in fs::read_dir(tree)? {
        let entry = entry?;
        if !preserve_git || entry.file_name() != ".git" {
            fs::rename(entry.path(), workspace.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn private_stage(parent: &Path, prefix: &str) -> io::Result<tempfile::TempDir> {
    let stage = tempfile::Builder::new().prefix(prefix).tempdir_in(parent)?;
    platform::private_directory(stage.path())?;
    Ok(stage)
}

fn prepare_snapshots_root(workspace: &Path, root: &Path) -> io::Result<PathBuf> {
    let root = restore_destination(root)?;
    ensure_disjoint(workspace, &root)?;
    platform::private_directory(&root)?;
    let root = existing_directory(&root)?;
    ensure_disjoint(workspace, &root)?;
    Ok(root)
}

fn restore_destination(path: &Path) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("workspace has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| invalid("workspace has no name"))?;
    let parent = existing_directory(parent)?;
    let destination = parent.join(name);
    if let Some(metadata) = entry_metadata(&destination)? {
        require_directory(&metadata)?;
    }
    Ok(destination)
}

fn snapshot_directory(root: &Path, id: &SnapshotId) -> io::Result<PathBuf> {
    let root = existing_directory(root)?;
    let snapshot = existing_directory(&root.join(id.as_str()))?;
    existing_directory(&snapshot.join("workspace"))?;
    Ok(snapshot)
}

fn existing_directory(path: &Path) -> io::Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    require_directory(&metadata)?;
    fs::canonicalize(path)
}

fn require_directory(metadata: &fs::Metadata) -> io::Result<()> {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid("checkpoint path is not a plain directory"));
    }
    Ok(())
}

fn ensure_disjoint(workspace: &Path, root: &Path) -> io::Result<()> {
    if workspace.starts_with(root) || root.starts_with(workspace) {
        return Err(invalid("workspace and snapshot roots overlap"));
    }
    Ok(())
}

fn write_metadata<T: Serialize>(root: &Path, metadata: &SnapshotMetadata<T>) -> io::Result<()> {
    let bytes = serde_json::to_vec(metadata).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(invalid("snapshot metadata exceeds its bound"));
    }
    let mut file = platform::private_file(&root.join("metadata.json"), FileAccess::CreateNew)?;
    file.write_all(&bytes)?;
    file.sync_all()
}

fn read_metadata<T: DeserializeOwned>(root: &Path) -> io::Result<SnapshotMetadata<T>> {
    let file = platform::private_file(&root.join("metadata.json"), FileAccess::Read)?;
    if file.metadata()?.len() > MAX_METADATA_BYTES {
        return Err(invalid("snapshot metadata exceeds its bound"));
    }
    let metadata: SnapshotMetadata<T> =
        serde_json::from_reader(file.take(MAX_METADATA_BYTES + 1)).map_err(io::Error::other)?;
    if metadata.format != FORMAT {
        return Err(invalid("unsupported snapshot format"));
    }
    Ok(metadata)
}

fn sync_tree(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            sync_tree(&entry?.path())?;
        }
        sync_directory(path)
    } else {
        fs::File::open(path)?.sync_all()
    }
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return platform::open_directory(path)?.sync_all();
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

pub(super) fn entry_metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn remove_entry(path: &Path) -> io::Result<()> {
    let Some(metadata) = entry_metadata(path)? else {
        return Ok(());
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
