//! Public file command values and local target selection.

use std::path::PathBuf;
use std::sync::Arc;

use openengine_cluster_protocol::FileRequest;
use openengine_cluster_server::identity::{
    BindingAttributes, ConnectionIdentity, ConnectionIdentityConfig, PrincipalId, TenantId,
};

use crate::native_v2_files::{DiskFileStore, SingleAccountFiles};
use super::NativeV2CliError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileCommand {
    pub target: Option<String>,
    pub request: FileRequest,
    pub transfer: FileCommandTransfer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileCommandTransfer {
    None,
    Upload(PathBuf),
    Download(PathBuf),
}

pub fn local_file_store() -> Result<(DiskFileStore, ConnectionIdentity, PathBuf), NativeV2CliError>
{
    let root = super::default_local_state_root()?;
    local_file_store_at(&root)
}

pub(crate) fn local_file_store_at(
    root: &std::path::Path,
) -> Result<(DiskFileStore, ConnectionIdentity, PathBuf), NativeV2CliError> {
    let identity = ConnectionIdentity::new(ConnectionIdentityConfig {
        principal: PrincipalId::new("local"),
        tenant: TenantId::new("local"),
        issued_at_ms: None,
        expires_at_ms: u64::MAX,
        binding_attributes: BindingAttributes::default(),
    });
    let store = DiskFileStore::open(
        root.join("files"),
        "local".to_owned(),
        Arc::new(SingleAccountFiles {
            principal: "local".to_owned(),
            tenant: "local".to_owned(),
        }),
    )?;
    let transfers = root.join("file-transfers");
    crate::native_v2_files::reclaim_transfers(&transfers)?;
    Ok((store, identity, transfers))
}

pub(crate) fn workspace_files(
    root: &std::path::Path,
) -> Result<crate::native_v2_files::WorkspaceFiles, NativeV2CliError> {
    let (host, identity, _) = local_file_store_at(root)?;
    Ok(crate::native_v2_files::WorkspaceFiles {
        host: Arc::new(host),
        identity,
        target: "local".into(),
        directory: root.join("workspace-inputs"),
    })
}

/// Scalar-only local runs need no file service or private input state.
pub(crate) fn input_files_if_present(
    root: &std::path::Path,
) -> Result<Option<crate::native_v2_files::WorkspaceFiles>, NativeV2CliError> {
    if !root
        .join("workspace-inputs/file-inputs")
        .try_exists()
        .map_err(|error| NativeV2CliError::Usage(error.to_string()))?
    {
        return Ok(None);
    }
    workspace_files(root).map(Some)
}

/// Refused uploads may exist before any accepted-input manifest has been created.
pub(crate) fn release_staged_uploads(
    root: &std::path::Path,
    schema: &openengine_cluster_protocol::PayloadType,
    value: &serde_json::Value,
) -> Result<(), NativeV2CliError> {
    if root.join("files").try_exists()? {
        workspace_files(root)?.release_staged_inputs(schema, value)?;
    }
    Ok(())
}
