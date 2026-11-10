//! Built-in run-global postmortem. Provider execution stays inside the original run's authority.

use std::sync::Arc;
use std::path::PathBuf;

use async_trait::async_trait;
use openengine_cluster_protocol::{RunId, TerminalResult};
use tokio::sync::watch;
use tokio::time::{Duration, Instant};

use crate::active_memory::{
    ActiveMemoryBackend, ActiveMemoryError, MemoryPublication, MemoryPublicationControl,
    MemorySnapshot, ScopeQuery, SuppliedMemoryManifest,
};
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_runner::{AuxiliaryRunnerFactory, NodeRunnerError};
use crate::native_v2_supervisor::RunEnvironment;
use crate::v2_run_ledger::{RunLedger, RunSnapshot};

mod context_budget;
mod definition;
mod evidence;
mod execution;
mod impact;
pub(crate) mod initialize;
mod plan;
mod proposals;
mod source_file;

pub(crate) const SETTLEMENT_GRACE: Duration = Duration::from_secs(30);

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
    pub timed_out: bool,
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
    #[error("postmortem work deadline elapsed")]
    TimedOut,
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
        let source = read_before_deadline(
            crate::active_memory::source_guard::SourceGuard::capture(
                &self.config.workspace,
                &latest.lessons,
            ),
            &work.cancellation,
            work.deadline,
        )
        .await?;
        let mut context = proposals::ProposalContext::new(
            &self.config,
            work.request,
            work.evidence,
            work.supplied.clone(),
        )
        .with_source(&source)
        .with_definition_file(work.definition.map(definition::DefinitionFile::path))
        .refreshed(latest)?;
        if attempt > 0 {
            context.include_findings(retry.initial, retry.previous)?;
        }
        let (report, mut units) = execution::run(execution::PostmortemExecution {
            runner: work.runner.clone(),
            plan: work.plan,
            request: work.request,
            context: &context,
            cancellation: work.cancellation.clone(),
            attempt,
            deadline: work.deadline,
        })
        .await?;
        if work
            .definition
            .is_some_and(|definition| !definition.unchanged())
        {
            return Err(PostmortemError::Preparation);
        }
        if units.iter().any(|unit| source_dependent(unit, latest))
            && !self.source_stable(work, &source).await
        {
            // Keep consolidation atomic even when a specialist mixes repository-specific
            // and unqualified graph cards: never retain only one side of upsert/retirement.
            units.retain(|unit| !source_dependent(unit, latest));
            execution::log(work.request, "Active Memory source changed or could not be reverified; source-dependent specialists deferred".to_owned()).await?;
        }
        Ok((report, completed_mutations(units)))
    }

    async fn source_stable(
        &self,
        work: &Reconciliation<'_>,
        source: &crate::active_memory::source_guard::SourceGuard,
    ) -> bool {
        source.reference().is_none()
            || read_before_deadline(
                source.unchanged(&self.config.workspace),
                &work.cancellation,
                work.deadline + SETTLEMENT_GRACE,
            )
            .await
            .unwrap_or(false)
    }

    async fn publish(
        &self,
        work: &Reconciliation<'_>,
        publication: MemoryPublication,
    ) -> Result<Option<usize>, PostmortemError> {
        if hard_cancelled(&work.cancellation) {
            return Err(PostmortemError::Cancelled);
        }
        let deadline = (work.deadline + SETTLEMENT_GRACE).min(Instant::now() + SETTLEMENT_GRACE);
        if Instant::now() >= deadline {
            return Err(PostmortemError::TimedOut);
        }
        let control = MemoryPublicationControl::new(work.cancellation.clone(), deadline);
        // The backend observes the ORIGINAL hard-cancellation receiver at its synchronous
        // dispatch gate; an asynchronously forwarded false channel would introduce a race.
        let result = self
            .config
            .store
            .publish(&work.request.run_id, publication, control)
            .await;
        self.publication_result(work, result).await
    }

    async fn publication_result(
        &self,
        work: &Reconciliation<'_>,
        result: Result<Vec<crate::active_memory::LessonVersion>, ActiveMemoryError>,
    ) -> Result<Option<usize>, PostmortemError> {
        match result {
            // A started write may commit while cancellation drains. Report that fact accurately.
            Ok(versions) => Ok(Some(versions.len())),
            Err(ActiveMemoryError::RevisionConflict { .. }) => {
                if hard_cancelled(&work.cancellation) {
                    return Err(PostmortemError::Cancelled);
                }
                execution::log(work.request, "Active Memory publication conflicted; retained findings require fresh reconciliation".to_owned()).await?;
                Ok(None)
            }
            Err(ActiveMemoryError::Cancelled) if hard_cancelled(&work.cancellation) => {
                Err(PostmortemError::Cancelled)
            }
            Err(ActiveMemoryError::Cancelled) => Err(PostmortemError::TimedOut),
            Err(_) => Err(PostmortemError::Storage),
        }
    }

    async fn refreshed(
        &self,
        work: &Reconciliation<'_>,
        previous: Option<&[crate::active_memory::Lesson]>,
    ) -> Result<MemorySnapshot, PostmortemError> {
        let scope = self.scope(work.request);
        let latest = read_before_deadline(
            self.current(work.request, &scope, work.cancellation.clone()),
            &work.cancellation,
            work.deadline,
        )
        .await??;
        if previous == Some(latest.lessons.as_slice()) {
            // A hidden tombstone or unchanged snapshot cannot supply new reconciliation input.
            return Err(PostmortemError::Storage);
        }
        Ok(latest)
    }

    async fn publish_report(
        &self,
        work: &Reconciliation<'_>,
        mut report: PostmortemReport,
        publication: MemoryPublication,
    ) -> Result<Option<PostmortemReport>, PostmortemError> {
        let published = match self.publish(work, publication).await {
            Ok(published) => published,
            Err(PostmortemError::Cancelled) => {
                report.cancelled = true;
                return Ok(Some(report));
            }
            Err(PostmortemError::TimedOut) => {
                report.timed_out = true;
                return Ok(Some(report));
            }
            Err(error) => return Err(error),
        };
        if let Some(count) = published {
            report.published_mutations = count;
            report.cancelled |= hard_cancelled(&work.cancellation);
            return Ok(Some(report));
        }
        if Instant::now() >= work.deadline {
            report.timed_out = true;
            execution::log(work.request, "Active Memory conflicted findings deferred after the total work deadline; no further provider calls started".to_owned()).await?;
            return Ok(Some(report));
        }
        Ok(None)
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
            if let Some(report) = work.stopped() {
                return Ok(report);
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
            if work.should_stop(&mut report) {
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
            if let Some(report) = self.publish_report(&work, report, publication).await? {
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
        let deadline = Instant::now()
            + Duration::from_millis(u64::from(
                request
                    .admitted
                    .runtime
                    .global_agents()
                    .postmortem
                    .timeout_ms,
            ));
        let result = self.run_inner(&request, cancellation, deadline).await;
        let report = match result {
            Err(PostmortemError::Cancelled) => PostmortemReport {
                cancelled: true,
                ..Default::default()
            },
            Err(PostmortemError::TimedOut) => PostmortemReport {
                timed_out: true,
                ..Default::default()
            },
            other => other?,
        };
        if report.cancelled || report.timed_out {
            request
                .ledger
                .append(
                    &request.run_id,
                    vec![crate::v2_run_ledger::RunEvent::RunTokenUsageObserved { usage: None }],
                )
                .await
                .map_err(|_| PostmortemError::Storage)?;
        }
        Ok(report)
    }
}

impl BuiltinPostmortem {
    async fn prepare(
        &self,
        request: &PostmortemRequest,
        cancellation: &watch::Receiver<bool>,
    ) -> Result<
        (
            AdmittedRun,
            evidence::FrozenEvidence,
            Option<definition::DefinitionFile>,
        ),
        PostmortemError,
    > {
        let plan = plan::build(&request.admitted).await?;
        let evidence = evidence::collect_cancellable(
            request,
            self.config.supplied.tool_observations(),
            cancellation,
        )
        .await?
        .ok_or(PostmortemError::Preparation)?;
        let definition =
            definition::DefinitionFile::prepare(&request.admitted, &self.config.workspace)?;
        Ok((plan, evidence, definition))
    }

    async fn run_inner(
        &self,
        request: &PostmortemRequest,
        cancellation: watch::Receiver<bool>,
        deadline: Instant,
    ) -> Result<PostmortemReport, PostmortemError> {
        if hard_cancelled(&cancellation) {
            return Err(PostmortemError::Cancelled);
        }
        let prepare = self.prepare(request, &cancellation);
        // No provider exists yet: read-only preparation is dropped promptly at either cutoff.
        let (plan, evidence, definition) =
            read_before_deadline(prepare, &cancellation, deadline).await??;
        let supplied = self.config.supplied.manifest();
        execution::record_manifest(request, self.config.snapshot.generation, supplied.as_ref())
            .await?;
        if hard_cancelled(&cancellation) {
            return Err(PostmortemError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(PostmortemError::TimedOut);
        }
        let runner = self.config.factory.fork(&plan)?;
        let result = self
            .reconcile(Reconciliation {
                request,
                plan: &plan,
                evidence: &evidence,
                supplied,
                runner: runner.clone(),
                cancellation,
                deadline,
                definition: definition.as_ref(),
            })
            .await;
        runner.close_run(&request.run_id).await;
        result
    }
}

async fn read_before_deadline<T>(
    read: impl std::future::Future<Output = T>,
    cancellation: &watch::Receiver<bool>,
    deadline: Instant,
) -> Result<T, PostmortemError> {
    tokio::select! {
        biased;
        () = super::backend::cancelled(cancellation.clone()) => Err(PostmortemError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(PostmortemError::TimedOut),
        result = read => Ok(result),
    }
}

fn hard_cancelled(cancellation: &watch::Receiver<bool>) -> bool {
    *cancellation.borrow() || cancellation.has_changed().is_err()
}

fn source_dependent(unit: &execution::CompletedSpecialist, snapshot: &MemorySnapshot) -> bool {
    unit.mutations
        .iter()
        .any(|mutation| source_mutation(mutation, snapshot))
}

fn completed_mutations(
    units: Vec<execution::CompletedSpecialist>,
) -> Vec<crate::active_memory::LessonMutation> {
    let mut mutations = std::collections::BTreeMap::new();
    for mutation in units.into_iter().flat_map(|unit| unit.mutations) {
        mutations
            .entry(mutation.id().to_owned())
            .or_insert(mutation);
    }
    mutations.into_values().collect()
}

fn source_mutation(
    mutation: &crate::active_memory::LessonMutation,
    snapshot: &MemorySnapshot,
) -> bool {
    match mutation {
        crate::active_memory::LessonMutation::Upsert { draft, .. } => {
            crate::active_memory::source_guard::source_bound(&draft.scope)
        }
        crate::active_memory::LessonMutation::Delete { id, .. } => snapshot
            .lessons
            .iter()
            .find(|lesson| lesson.id == *id)
            .is_none_or(|lesson| crate::active_memory::source_guard::source_bound(&lesson.scope)),
    }
}

struct Reconciliation<'a> {
    request: &'a PostmortemRequest,
    plan: &'a AdmittedRun,
    evidence: &'a evidence::FrozenEvidence,
    supplied: Option<SuppliedMemoryManifest>,
    runner: Arc<dyn crate::native_v2_runner::NodeRunner>,
    cancellation: watch::Receiver<bool>,
    deadline: Instant,
    definition: Option<&'a definition::DefinitionFile>,
}

impl Reconciliation<'_> {
    fn stopped(&self) -> Option<PostmortemReport> {
        if hard_cancelled(&self.cancellation) {
            Some(PostmortemReport {
                cancelled: true,
                ..Default::default()
            })
        } else if Instant::now() >= self.deadline {
            Some(PostmortemReport {
                timed_out: true,
                ..Default::default()
            })
        } else {
            None
        }
    }

    fn should_stop(&self, report: &mut PostmortemReport) -> bool {
        report.cancelled |= hard_cancelled(&self.cancellation);
        // Successful roles reconcile all batches, including on retries; another incomplete
        // specialist cannot invalidate an independently sealed scope's transaction.
        // Empty review batches still check the snapshot CAS, so concurrent changes to
        // kept cards trigger the same reconciliation as a mutating batch.
        report.cancelled
    }
}

struct RetryContext<'a> {
    initial: &'a [crate::active_memory::LessonMutation],
    previous: &'a [crate::active_memory::LessonMutation],
    attempt: u64,
}
