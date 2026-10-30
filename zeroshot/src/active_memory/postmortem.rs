//! Built-in run-global postmortem. Provider execution stays inside the original run's authority.

use std::sync::Arc;
use std::path::PathBuf;

use async_trait::async_trait;
use openengine_cluster_protocol::{RunId, TerminalResult};
use tokio::sync::watch;

use crate::active_memory::{
    ActiveMemoryBackend, ActiveMemoryError, MemoryPublication, MemorySnapshot, ScopeQuery,
    SuppliedMemoryManifest,
};
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
    #[error("postmortem cancelled")]
    Cancelled,
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
    pub store: Arc<dyn ActiveMemoryBackend>,
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

    fn scope(&self, request: &PostmortemRequest) -> ScopeQuery {
        let config = request.admitted.runtime.active_memory();
        ScopeQuery {
            repository: self.config.repository.clone(),
            graph_hash: Some(self.config.graph_hash.clone()),
            node_id: None,
            repository_lessons: config.repository,
            graph_lessons: config.graph,
        }
    }

    async fn current(
        &self,
        request: &PostmortemRequest,
        scope: &ScopeQuery,
        cancellation: watch::Receiver<bool>,
    ) -> Result<MemorySnapshot, PostmortemError> {
        let latest = tokio::select! {
            biased;
            () = super::backend::cancelled(cancellation) => return Err(PostmortemError::Cancelled),
            result = self.config.store.snapshot(&request.run_id, scope) =>
                result.map_err(|_| PostmortemError::Storage)?,
        };
        if latest
            .lessons
            .iter()
            .any(|lesson| !lesson.scope.matches(scope))
        {
            return Err(PostmortemError::Storage);
        }
        Ok(latest)
    }

    async fn execute(
        &self,
        work: &Reconciliation<'_>,
        latest: &MemorySnapshot,
        retry: RetryContext<'_>,
    ) -> Result<(PostmortemReport, Vec<crate::active_memory::LessonMutation>), PostmortemError>
    {
        let attempt = retry.attempt;
        let mut context = proposals::ProposalContext::new(
            &self.config,
            work.request,
            work.evidence,
            work.supplied.clone(),
        )
        .refreshed(latest)?;
        if attempt > 0 {
            context.include_findings(retry.initial, retry.previous)?;
        }
        execution::run(execution::PostmortemExecution {
            runner: work.runner.clone(),
            plan: work.plan,
            request: work.request,
            context: &context,
            cancellation: work.cancellation.clone(),
            attempt,
        })
        .await
    }

    async fn publish(
        &self,
        request: &PostmortemRequest,
        publication: MemoryPublication,
        cancellation: watch::Receiver<bool>,
    ) -> Result<Option<usize>, PostmortemError> {
        let result = self
            .config
            .store
            .publish(&request.run_id, publication, cancellation.clone())
            .await;
        if *cancellation.borrow() || cancellation.has_changed().is_err() {
            return Err(PostmortemError::Cancelled);
        }
        match result {
            Ok(versions) => Ok(Some(versions.len())),
            Err(ActiveMemoryError::RevisionConflict { .. }) => {
                execution::log(request, "Active Memory publication conflicted; refreshing lessons and reconciling retained findings".to_owned()).await?;
                Ok(None)
            }
            Err(ActiveMemoryError::Cancelled) => Err(PostmortemError::Cancelled),
            Err(_) => Err(PostmortemError::Storage),
        }
    }

    async fn refreshed(
        &self,
        work: &Reconciliation<'_>,
        previous: Option<&[crate::active_memory::Lesson]>,
    ) -> Result<MemorySnapshot, PostmortemError> {
        let latest = self
            .current(
                work.request,
                &self.scope(work.request),
                work.cancellation.clone(),
            )
            .await?;
        if previous == Some(latest.lessons.as_slice()) {
            // A hidden tombstone or unchanged snapshot cannot supply new reconciliation input.
            return Err(PostmortemError::Storage);
        }
        Ok(latest)
    }

    async fn reconcile(
        &self,
        work: Reconciliation<'_>,
    ) -> Result<PostmortemReport, PostmortemError> {
        let scope = self.scope(work.request);
        let mut discoveries: Option<Vec<crate::active_memory::LessonMutation>> = None;
        let mut attempt = 0_u64;
        let mut previous = Vec::new();
        let mut conflicted_lessons = None;
        loop {
            if *work.cancellation.borrow() {
                return Ok(PostmortemReport {
                    cancelled: true,
                    ..Default::default()
                });
            }
            let latest = self.refreshed(&work, conflicted_lessons.as_deref()).await?;
            let (mut report, mutations) = self
                .execute(
                    &work,
                    &latest,
                    RetryContext {
                        initial: discoveries.as_deref().unwrap_or_default(),
                        previous: &previous,
                        attempt,
                    },
                )
                .await?;
            if work.should_stop(&mut report, attempt)? {
                return Ok(report);
            }
            discoveries.get_or_insert_with(|| mutations.clone());
            let publication = MemoryPublication {
                scope: scope.clone(),
                reviewed: latest
                    .lessons
                    .iter()
                    .map(crate::active_memory::Lesson::version)
                    .collect(),
                mutations: mutations.clone(),
            };
            if let Some(count) = self
                .publish(work.request, publication, work.cancellation.clone())
                .await?
            {
                report.published_mutations = count;
                return Ok(report);
            }
            conflicted_lessons = Some(latest.lessons);
            previous = mutations;
            attempt = attempt.checked_add(1).ok_or(PostmortemError::Storage)?;
        }
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
        let Some(evidence) = evidence::collect_cancellable(
            &request,
            self.config.supplied.tool_observations(),
            &cancellation,
        )
        .await?
        else {
            return Ok(PostmortemReport {
                cancelled: true,
                ..Default::default()
            });
        };
        let supplied = self.config.supplied.manifest();
        execution::record_manifest(&request, self.config.snapshot.generation, supplied.as_ref())
            .await?;
        let runner = self.config.factory.fork(&plan)?;
        let result = self
            .reconcile(Reconciliation {
                request: &request,
                plan: &plan,
                evidence: &evidence,
                supplied,
                runner: runner.clone(),
                cancellation,
            })
            .await;
        runner.close_run(&request.run_id).await;
        match result {
            Err(PostmortemError::Cancelled) => Ok(PostmortemReport {
                cancelled: true,
                ..Default::default()
            }),
            result => result,
        }
    }
}

struct Reconciliation<'a> {
    request: &'a PostmortemRequest,
    plan: &'a AdmittedRun,
    evidence: &'a evidence::FrozenEvidence,
    supplied: Option<SuppliedMemoryManifest>,
    runner: Arc<dyn crate::native_v2_runner::NodeRunner>,
    cancellation: watch::Receiver<bool>,
}

impl Reconciliation<'_> {
    fn should_stop(
        &self,
        report: &mut PostmortemReport,
        attempt: u64,
    ) -> Result<bool, PostmortemError> {
        report.cancelled |= *self.cancellation.borrow();
        if report.cancelled {
            return Ok(true);
        }
        if attempt > 0 && report.failed_roles > 0 {
            return Err(PostmortemError::Preparation);
        }
        Ok(false)
    }
}

struct RetryContext<'a> {
    initial: &'a [crate::active_memory::LessonMutation],
    previous: &'a [crate::active_memory::LessonMutation],
    attempt: u64,
}
