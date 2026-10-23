//! Built-in run-global postmortem. Provider execution stays inside the original run's authority.

use std::sync::Arc;
use std::path::PathBuf;

use async_trait::async_trait;
use openengine_cluster_protocol::{RunId, TerminalResult};
use tokio::sync::{Mutex, watch};

use crate::active_memory::{ActiveMemoryStore, MemorySnapshot, SuppliedMemoryManifest};
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_runner::{AuxiliaryRunnerFactory, NodeRunnerError};
use crate::native_v2_supervisor::RunEnvironment;
use crate::v2_run_ledger::{RunLedger, RunSnapshot};

mod context_budget;
mod evidence;
mod execution;
mod impact;
pub(crate) mod initialize;
mod plan;
mod proposals;
mod source_file;

#[cfg(test)]
mod tests;

/// Returns only cards actually returned through an injection carrier, not merely eligible cards.
/// `None` means delivery evidence is unavailable, and maintenance must abstain.
pub trait SuppliedLessonSource: Send + Sync {
    fn manifest(&self) -> Option<SuppliedMemoryManifest>;
    fn tool_observations(&self) -> Vec<crate::active_memory::ToolObservation> {
        Vec::new()
    }
}

pub struct PostmortemRequest {
    pub run_id: RunId,
    pub admitted: AdmittedRun,
    pub snapshot: RunSnapshot,
    pub primary_terminal: TerminalResult,
    pub environment: Arc<RunEnvironment>,
    pub ledger: Arc<dyn RunLedger>,
}

/// Read-only durable facts, without provider credentials or execution authority.
#[derive(Clone, Copy)]
pub(super) struct MainRunFacts<'a> {
    pub run_id: &'a RunId,
    pub admitted: &'a AdmittedRun,
    pub snapshot: &'a RunSnapshot,
    pub primary_terminal: &'a TerminalResult,
    pub ledger: &'a dyn RunLedger,
}

impl<'a> From<&'a PostmortemRequest> for MainRunFacts<'a> {
    fn from(request: &'a PostmortemRequest) -> Self {
        Self {
            run_id: &request.run_id,
            admitted: &request.admitted,
            snapshot: &request.snapshot,
            primary_terminal: &request.primary_terminal,
            ledger: request.ledger.as_ref(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PostmortemReport {
    pub completed_roles: usize,
    pub failed_roles: usize,
    pub published_mutations: usize,
    pub cancelled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PostmortemError {
    #[error("{0}")]
    Unavailable(&'static str),
    #[error("postmortem provider cleanup could not be confirmed")]
    CleanupUnconfirmed,
    #[error("postmortem could not be prepared")]
    Preparation,
    #[error("postmortem evidence or publication is unavailable")]
    Storage,
}

impl From<NodeRunnerError> for PostmortemError {
    fn from(error: NodeRunnerError) -> Self {
        if error == NodeRunnerError::CleanupUnconfirmed {
            Self::CleanupUnconfirmed
        } else {
            Self::Preparation
        }
    }
}

#[async_trait]
pub trait RunPostmortem: Send + Sync {
    /// Cancellation must stop new work and drain already-started provider cleanup before return.
    async fn run(
        &self,
        request: PostmortemRequest,
        cancellation: watch::Receiver<bool>,
    ) -> Result<PostmortemReport, PostmortemError>;
}

pub struct BuiltinPostmortemConfig {
    pub store: Arc<Mutex<ActiveMemoryStore>>,
    pub factory: Arc<dyn AuxiliaryRunnerFactory>,
    pub snapshot: MemorySnapshot,
    pub supplied: Arc<dyn SuppliedLessonSource>,
    pub repository: Option<String>,
    pub graph_hash: String,
    pub workspace: PathBuf,
}

pub struct BuiltinPostmortem {
    config: BuiltinPostmortemConfig,
}

impl BuiltinPostmortem {
    #[must_use]
    pub fn new(config: BuiltinPostmortemConfig) -> Self {
        Self { config }
    }

    async fn publish(
        &self,
        mutations: Vec<crate::active_memory::LessonMutation>,
        cancellation: watch::Receiver<bool>,
    ) -> Result<usize, PostmortemError> {
        if mutations.is_empty() || *cancellation.borrow() {
            return Ok(0);
        }
        // Ownership enters the blocking task, so cancellation never releases an in-flight DB lock.
        let mut store = self.config.store.clone().lock_owned().await;
        if *cancellation.borrow() {
            return Ok(0);
        }
        tokio::task::spawn_blocking(move || {
            if *cancellation.borrow() {
                return Ok(Vec::new());
            }
            store.apply_batch(&mutations)
        })
        .await
        .map_err(|_| PostmortemError::Storage)?
        .map(|versions| versions.len())
        .map_err(|_| PostmortemError::Storage)
    }
}

#[async_trait]
impl RunPostmortem for BuiltinPostmortem {
    async fn run(
        &self,
        request: PostmortemRequest,
        cancellation: watch::Receiver<bool>,
    ) -> Result<PostmortemReport, PostmortemError> {
        if *cancellation.borrow() {
            return Ok(PostmortemReport {
                cancelled: true,
                ..Default::default()
            });
        }
        let plan = plan::build(&request.admitted).await?;
        let evidence =
            evidence::collect(&request, self.config.supplied.tool_observations()).await?;
        let supplied = self.config.supplied.manifest();
        execution::record_manifest(&request, self.config.snapshot.generation, supplied.as_ref())
            .await?;
        let context = proposals::ProposalContext::new(&self.config, &request, &evidence, supplied);
        let runner = self.config.factory.fork(&plan)?;
        let publication_cancel = cancellation.clone();
        let result = execution::run(execution::PostmortemExecution {
            runner: runner.clone(),
            plan: &plan,
            request: &request,
            context: &context,
            cancellation,
        })
        .await;
        runner.close_run(&request.run_id).await;
        let (mut report, mutations) = result?;
        if !report.cancelled {
            report.published_mutations =
                self.publish(mutations, publication_cancel.clone()).await?;
        }
        report.cancelled |= *publication_cancel.borrow();
        Ok(report)
    }
}
