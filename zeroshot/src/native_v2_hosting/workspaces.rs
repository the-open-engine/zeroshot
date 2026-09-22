//! Host-owned durable workspace storage at the native allocation boundary.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{NodeName, RunId};

use crate::full_v1_reducer::DurableExecution;
use crate::native_v2_supervisor::checkpoints::{CheckpointError, RunCheckpointStore};

/// Trusted allocation facts. Called after exact source checkout, before any worker starts.
pub struct HostedWorkspaceRequest<'a> {
    pub run_id: &'a RunId,
    pub workspace: &'a Path,
    pub checkpoint_directory: &'a Path,
    pub writers: BTreeSet<NodeName>,
}

/// Restored execution authority and the store that owns subsequent engine barriers.
pub struct HostedWorkspace {
    pub checkpoints: Arc<dyn RunCheckpointStore>,
    pub execution_seed: Vec<DurableExecution>,
    /// The original delivery identity, when restoring a successor in an existing lineage.
    pub delivery_run_id: Option<RunId>,
}

/// An installed host may replace local checkpoint storage with its durable backend.
///
/// The host resolves its own authenticated restore selection; callers never supply filesystem
/// paths or execution history. Restore must preserve the workspace root, finish before returning,
/// and leave no unowned background process writing into the workspace on cancellation.
#[async_trait]
pub trait HostedWorkspaceStorage: Send + Sync {
    async fn prepare(
        &self,
        request: HostedWorkspaceRequest<'_>,
    ) -> Result<HostedWorkspace, CheckpointError>;
}
