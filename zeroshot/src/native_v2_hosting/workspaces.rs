//! Host-owned durable workspace storage at the native allocation boundary.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{NodeName, RunId};

use crate::full_v1_reducer::DurableExecution;

pub use crate::native_v2_runner::conversations::ConversationFiles;
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
    /// Authenticated immediate predecessor, when the host restores a successor.
    /// The runtime appends its continuation after taking over the supplied vector record.
    pub resumed_from: Option<RunId>,
    pub vector_seed: Vec<openengine_cluster_protocol::RunVector>,
    /// Durable comment effects, independent of restored workspace bytes.
    pub comment_publications:
        Option<Arc<dyn crate::native_v2_comment_delivery::CommentPublicationStore>>,
    pub checkpoints: Arc<dyn RunCheckpointStore>,
    pub execution_seed: Vec<DurableExecution>,
    /// The original delivery identity, when restoring a successor in an existing lineage.
    pub delivery_run_id: Option<RunId>,
}

/// An installed host may replace local checkpoint storage with its durable backend.
///
/// The returned `RunCheckpointStore` optionally receives `ConversationFiles` through
/// `retain_conversations` before each checkpoint. Store the complete files outside the workspace
/// tree served by export-workspace, and return the selected files through `restored_conversations`
/// on restore, including continuations that retain their workspace. Default methods keep older
/// hosts working: their successors start new conversations with a system log explanation.
///
/// The host resolves its own authenticated restore selection; callers never supply filesystem
/// paths or execution history. Restore must preserve the workspace root, finish before returning,
/// and leave no unowned background process writing into the workspace on cancellation.
#[async_trait]
pub trait HostedWorkspaceStorage: Send + Sync {
    /// Retain acknowledged predecessor receipts before the runtime claims its successor.
    /// Use the same lineage backend returned by `prepare`, with monotonic generation checks.
    async fn retain_delivery_state(
        &self,
        run_id: &RunId,
        state: &crate::native_v2_contract::DeliveryCheckpoint,
    ) -> Result<(), CheckpointError>;

    async fn prepare(
        &self,
        request: HostedWorkspaceRequest<'_>,
    ) -> Result<HostedWorkspace, CheckpointError>;
}
