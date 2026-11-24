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
