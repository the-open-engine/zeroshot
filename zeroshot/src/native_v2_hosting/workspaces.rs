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
    /// Authenticated immediate predecessor, when the host restores a successor.
    /// The runtime appends its continuation after taking over the supplied vector record.
    pub resumed_from: Option<RunId>,
    pub vector_seed: Vec<openengine_cluster_protocol::RunVector>,
    /// Durable comment effects, independent of restored workspace bytes.
    pub comment_publications:
        Option<Arc<dyn crate::native_v2_comment_delivery::CommentPublicationStore>>,
    pub checkpoints: Arc<dyn RunCheckpointStore>,
    pub execution_seed: Vec<DurableExecution>,
    /// Validated answer or explicit expiry for the pending execution restored by the host.
    pub request_continuation: Option<HostedRequestContinuation>,
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

/// Host-authenticated selection paired with the request checkpoint's pending history.
/// Hosts can call `validate` before allocating a workspace; late answers are rejected.
#[derive(Clone, Debug)]
pub struct HostedRequestContinuation {
    pub suspension: openengine_cluster_protocol::RunSuspension,
    pub from: openengine_cluster_protocol::RunResumeFrom,
}

impl HostedRequestContinuation {
    pub fn validate(
        &self,
        now: openengine_cluster_protocol::UnixTimestampMillis,
    ) -> Result<openengine_cluster_protocol::RequestResolution, String> {
        let resolution =
            crate::native_v2_request::resolve(&self.suspension, Some(&self.from), now)?;
        if matches!(
            self.from,
            openengine_cluster_protocol::RunResumeFrom::Answer { .. }
        ) && matches!(
            resolution,
            openengine_cluster_protocol::RequestResolution::Expiry {}
        ) {
            return Err("request expired; select expiry explicitly".to_owned());
        }
        Ok(resolution)
    }
}

impl HostedWorkspace {
    pub(crate) fn settle_request(
        &mut self,
        run_id: &RunId,
        now: openengine_cluster_protocol::UnixTimestampMillis,
    ) -> Result<(), String> {
        use crate::full_v1_reducer::{DurableExecutionState, HistoryPosition};
        use crate::native_v2_request::RequestOutcome;
        let Some(continuation) = self.request_continuation.take() else {
            return Ok(());
        };
        if self.resumed_from.is_none() {
            return Err("request continuation requires its predecessor".to_owned());
        }
        let resolution = continuation.validate(now)?;
        let openengine_cluster_protocol::RunSuspension::Request { node, request, .. } =
            continuation.suspension
        else {
            return Err("request suspension required".to_owned());
        };
        let position = self
            .execution_seed
            .iter()
            .map(|execution| match &execution.state {
                DurableExecutionState::Settled { position, .. }
                | DurableExecutionState::Voided { position, .. } => {
                    position.get().max(execution.dispatch_position.get())
                }
                DurableExecutionState::Active => execution.dispatch_position.get(),
            })
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("history position overflow")?;
        let pending: Vec<_> = self
            .execution_seed
            .iter_mut()
            .filter(|execution| matches!(execution.state, DurableExecutionState::Active))
            .collect();
        if pending.len() != 1 || pending[0].occurrence.node != node {
            return Err("request checkpoint must contain its one pending execution".to_owned());
        }
        let execution = &pending[0];
        crate::v2_run_ledger::validate_event(&crate::v2_run_ledger::RunEvent::NodeCompleted {
            completion: crate::native_v2_contract::NodeCompletion {
                reference: crate::native_v2_contract::ExecutionRef {
                    run_id: run_id.clone(),
                    node: node.clone(),
                    node_instance: execution.node_instance,
                    execution: execution.execution,
                },
                outcome: resolution.outcome(),
            },
        })
        .map_err(|error| format!("$.output: {error}"))?;
        let vector = crate::v2_run_ledger::vector_event(
            run_id,
            crate::native_v2_request::resolution_vector(&request, &resolution),
        )
        .map_err(|error| error.to_string())?;
        crate::v2_run_ledger::validate_event(&vector)
            .map_err(|error| format!("$.output: {error}"))?;
        for execution in pending {
            execution.state = DurableExecutionState::Settled {
                position: HistoryPosition::new(position).map_err(|error| error.to_string())?,
                outcome: resolution.outcome(),
            };
        }
        if let crate::v2_run_ledger::RunEvent::Vector { vector } = vector {
            self.vector_seed.push(vector);
        }
        Ok(())
    }
}
