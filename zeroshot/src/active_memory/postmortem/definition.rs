//! Complete authored authority, inline for ordinary definitions and private files for large ones.

use std::path::{Path, PathBuf};
use serde_json::{Value, json};
use crate::native_v2_contract::AdmittedRun;
use super::PostmortemError;

pub(super) const INLINE_BYTES: usize = 64 * 1024;

pub(super) fn document(admitted: &AdmittedRun) -> Value {
    // Runtime plans contain connection references, never resolved provider/environment values.
    json!({"authored_graph":admitted.graph,"authored_runtime":admitted.runtime})
}

pub(super) fn reference(evidence: &mut Value, path: &Path) {
    if let Some(object) = evidence.as_object_mut() {
        object.remove("authored_graph");
        object.remove("authored_runtime");
        object.insert("authored_definition_file".to_owned(), json!(path));
    }
}

struct DefinitionDirectory(PathBuf);

impl DefinitionDirectory {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for DefinitionDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) struct DefinitionFile {
    directory: DefinitionDirectory,
    path: PathBuf,
    bytes: Vec<u8>,
}

impl DefinitionFile {
    pub(super) fn prepare(
        admitted: &AdmittedRun,
        workspace: &Path,
    ) -> Result<Option<Self>, PostmortemError> {
        let document = document(admitted);
        if super::context_budget::encoded_size(&document) <= INLINE_BYTES {
            return Ok(None);
        }
        let bytes = serde_json::to_vec(&document).map_err(|_| PostmortemError::Preparation)?;
        let directory =
            definition_directory(workspace).map_err(|_| PostmortemError::Preparation)?;
        assign_reader(directory.path(), workspace).map_err(|_| PostmortemError::Preparation)?;
        let path =
            write_definition(directory.path(), &bytes).map_err(|_| PostmortemError::Preparation)?;
        let mut permissions = std::fs::metadata(&path)
            .map_err(|_| PostmortemError::Preparation)?
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).map_err(|_| PostmortemError::Preparation)?;
        Ok(Some(Self {
            directory,
            path,
            bytes,
        }))
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn unchanged(&self) -> bool {
        use std::io::Read;
        crate::execution::platform::open_readonly_file(&self.path)
            .and_then(|mut file| {
                let mut bytes = Vec::new();
                file.by_ref()
                    .take(self.bytes.len() as u64 + 1)
                    .read_to_end(&mut bytes)?;
                Ok(bytes == self.bytes)
            })
            .unwrap_or(false)
    }
}

impl Drop for DefinitionFile {
    fn drop(&mut self) {
        // Windows cannot remove a read-only file; the private directory remains pinned until here.
        #[cfg(windows)]
        if let Ok(metadata) = std::fs::metadata(&self.path) {
            let mut permissions = metadata.permissions();
            #[expect(
                clippy::permissions_set_readonly_false,
                reason = "Windows-only: ACL permissions remain unchanged"
            )]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(&self.path, permissions);
        }
        let _ = self.directory.path();
    }
}

fn private_directory(parent: &Path) -> std::io::Result<DefinitionDirectory> {
    let path = parent.join(format!(
        ".active-memory-definition-{}",
        uuid::Uuid::now_v7()
    ));
    crate::execution::platform::create_private_directory(&path)?;
    Ok(DefinitionDirectory(path))
}

fn definition_directory(workspace: &Path) -> std::io::Result<DefinitionDirectory> {
    let parent = workspace
        .parent()
        .ok_or_else(|| std::io::Error::other("workspace has no parent"))?;
    match private_directory(parent) {
        Ok(directory) => Ok(directory),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
            ) && same_reader(workspace)? =>
        {
            local_temporary_directory(workspace, error)
        }
        Err(error) => Err(error),
    }
}

// A contained reader with another UID retains the known-traversable run parent.
fn local_temporary_directory(
    workspace: &Path,
    original: std::io::Error,
) -> std::io::Result<DefinitionDirectory> {
    let temporary = std::env::temp_dir().canonicalize()?;
    if temporary.starts_with(workspace.canonicalize()?) {
        return Err(original);
    }
    private_directory(&temporary)
}

fn same_reader(workspace: &Path) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(std::fs::metadata(workspace)?.uid() == unsafe { libc::geteuid() })
    }
    #[cfg(not(unix))]
    {
        // Windows providers are local and retain the invoking user's identity.
        let _ = workspace;
        Ok(true)
    }
}

fn assign_reader(directory: &Path, workspace: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = std::fs::metadata(workspace)?;
        if std::fs::metadata(directory)?.uid() != owner.uid() {
            std::os::unix::fs::lchown(directory, Some(owner.uid()), Some(owner.gid()))?;
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (directory, workspace);
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(not(windows))]
fn write_definition(directory: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    crate::native_v2_capsule::provider_process::write_provider_file(
        directory,
        "definition.json",
        bytes,
    )
}

#[cfg(windows)]
fn write_definition(directory: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let path = directory.join("definition.json");
    let mut file = crate::execution::platform::private_file(
        &path,
        crate::execution::platform::FileAccess::CreateNew,
    )?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(path)
}
