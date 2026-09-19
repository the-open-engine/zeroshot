//! Transport-neutral cloud controller for one native-v2 run per disposable capsule.
//!
//! Admission, durable graph truth, supervision, observation, environment selection, and terminal
//! policy remain controller-owned. The allocator supplies exactly one opaque node runner, one
//! liveness signal, and one result-bearing cleanup authority for the run's capsule/workspace.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ClusterStatus, EnumLabel, GetParams, GetResult, GraphProfile, GraphProfileSet,
    InitializeParams, InitializeResult, RunAttachEventNotification, RunAttachParams,
    RunAttachResult, RunForceParams, RunForceResult, RunId, RunListParams, RunListResult,
    RunDiscardWorkspaceParams, RunDiscardWorkspaceResult, RunLogEventNotification, RunLogsParams,
    RunLogsResult, RunResumeParams, RunResumeResult, RunStatus, RunStatusParams, RunStatusResult,
    RunSubmitParams, RunSubmitResult, RunWatchEventNotification, RunWatchParams, RunWatchResult,
    ServerCapabilities, Sha256Digest, SubscriptionCloseReason, TerminalResult, WorkerErrorCode,
    WorkerOutcome, GONE, GRAPH_INVALID, IDEMPOTENCY_REUSE, INTERNAL_ERROR_CODE, NOT_FOUND,
};
use openengine_cluster_server::native_v2::{
    RunAttachEventStream, RunLogEventStream, RunSubscriptionItem, RunSubscriptionSource,
    RunSubscriptionStream, RunWatchEventStream,
};
use openengine_cluster_server::{BackendError, ClusterBackend, ConnectionContext};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{watch, Mutex};

use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission, NativeV2AdmissionError};
use crate::native_v2_contract::{AdmittedRun, NodeCompletion, RunSubmission};
#[cfg(test)]
use crate::native_v2_contract::EnvironmentVariableName;
use crate::native_v2_observability::{
    NativeV2Observability, NativeV2ObservationError, RunAttachSubscription, RunLogsSubscription,
    RunWatchSubscription,
};
use crate::native_v2_portable_controller::{
    PortableRunEngine, PortableRunEngineBootstrap, PortableRuntime,
};
use crate::native_v2_runner::NodeRunner;
use crate::native_v2_supervisor::{
    NativeV2SupervisorError, RunEnvironmentError, RunRuntimeExit, RuntimeCleanupUnavailable,
};
use crate::v2_run_ledger::{
    CreateRun, CreateRunOutcome, RunEvent, RunLedger, RunLedgerError, RunSummary, StoredRun,
};

#[cfg(test)]
#[path = "native_v2_cloud/tests.rs"]
mod tests;

mod contracts;
pub use contracts::{
    AllocatedCapsule, CapsuleAllocationUnavailable, CapsuleAllocator, CapsuleCleanup,
    CapsuleCleanupUnavailable, CapsuleDestroyed, CloudRunReceipt, ControllerClaimUnavailable,
    ExclusiveControllerClaim, RetainedAllocationRequest, RetainedAllocationUnavailable,
};
pub use crate::native_v2_supervisor::RunEnvironment;

/// Compatibility name for the protocol-owned submission while downstream callers migrate.
pub type CloudRunSubmission = RunSubmission;

#[derive(Debug, Error)]
pub enum NativeV2CloudError {
    #[error(transparent)]
    ControllerClaim(#[from] ControllerClaimUnavailable),
    #[error(transparent)]
    Admission(#[from] NativeV2AdmissionError),
    #[error(transparent)]
    Ledger(#[from] RunLedgerError),
    #[error(transparent)]
    Observation(#[from] NativeV2ObservationError),
    #[error(transparent)]
    Allocation(#[from] CapsuleAllocationUnavailable),
    #[error(transparent)]
    Supervisor(#[from] NativeV2SupervisorError),
    #[error(transparent)]
    Environment(#[from] RunEnvironmentError),
    #[error("submission identity could not be constructed")]
    SubmissionIdentity,
    #[error("resume credential resolution is invalid")]
    ResumeCredentials,
}

#[derive(Clone)]
pub struct NativeV2CloudController {
    ledger: Arc<dyn RunLedger>,
    allocator: Arc<dyn CapsuleAllocator>,
    observability: NativeV2Observability,
    runtimes: Arc<Mutex<BTreeMap<RunId, RuntimeSlot>>>,
    submission_turn: Arc<Mutex<()>>,
    reconstructed_turn: Arc<Mutex<()>>,
    delivery_policy: DeliveryPolicy,
    operator_diagnostics: Arc<crate::native_v2_target_authority::OperatorDiagnosticStore>,
}

#[derive(Clone)]
enum RuntimeSlot {
    Running(Arc<PortableRunEngine>),
}

enum ForceTarget {
    Terminal,
    Running(Arc<PortableRunEngine>),
    Reconstructed,
}

struct RunSecretEnvelope {
    environment: Arc<RunEnvironment>,
    github_token: Option<String>,
}

struct ResumeSecretRequest<'a> {
    admitted: &'a AdmittedRun,
    successor_run_id: &'a RunId,
    connections: openengine_cluster_protocol::RunConnectionValues,
    connection_resolver: Option<openengine_cluster_protocol::TargetConnectionResolver>,
    github_token: Option<String>,
}

struct AllocatedRunStart {
    stored: StoredRun,
    environment: Arc<RunEnvironment>,
    controller_claim: Arc<dyn ExclusiveControllerClaim>,
    capsule: AllocatedCapsule,
}

impl NativeV2CloudController {
    /// Claims exclusive target authority, reconciles durable nonterminal runs, then constructs
    /// the target adapter. Each submitted run carries its own immutable runtime plan.
    pub async fn new(
        ledger: Arc<dyn RunLedger>,
        allocator: Arc<dyn CapsuleAllocator>,
    ) -> Result<Self, NativeV2CloudError> {
        Self::new_with_delivery_policy(ledger, allocator, DeliveryPolicy::Required).await
    }

    pub async fn new_with_delivery_policy(
        ledger: Arc<dyn RunLedger>,
        allocator: Arc<dyn CapsuleAllocator>,
        delivery_policy: DeliveryPolicy,
    ) -> Result<Self, NativeV2CloudError> {
        let controller = Self {
            observability: NativeV2Observability::new(ledger.clone()),
            ledger,
            allocator,
            runtimes: Arc::new(Mutex::new(BTreeMap::new())),
            submission_turn: Arc::new(Mutex::new(())),
            reconstructed_turn: Arc::new(Mutex::new(())),
            delivery_policy,
            operator_diagnostics: Arc::default(),
        };
        controller.reconcile_persisted_runs().await?;
        Ok(controller)
    }

    pub(crate) fn with_operator_diagnostics(
        mut self,
        diagnostics: Arc<crate::native_v2_target_authority::OperatorDiagnosticStore>,
    ) -> Self {
        self.operator_diagnostics = diagnostics;
        self
    }

    /// Reconciles durable nonterminal truth before this controller can serve any OECP method.
    /// A replacement runtime is never allocated during startup.
    async fn reconcile_persisted_runs(&self) -> Result<(), NativeV2CloudError> {
        for summary in self.ledger.list().await? {
            let stored = self
                .ledger
                .get(&summary.run_id)
                .await?
                .ok_or(RunLedgerError::RunNotFound)?;
            if stored.snapshot.terminal.is_some() {
                continue;
            }
            let _claim = self.allocator.claim_controller(&summary.run_id).await?;
            self.allocator
                .destroy_or_confirm_absent(&summary.run_id, RunRuntimeExit::RuntimeLost)
                .await
                .map_err(|_| NativeV2SupervisorError::RuntimeCleanup(RuntimeCleanupUnavailable))?;
            append_runtime_lost(self.ledger.as_ref(), &stored).await?;
        }
        Ok(())
    }

    /// Admits before every durable or allocation effect. Run identity is assigned by the host and
    /// exact resubmissions retain that identity without allocating a replacement runtime.
    pub async fn submit(
        &self,
        request: RunSubmitParams,
    ) -> Result<CloudRunReceipt, NativeV2CloudError> {
        let environment = RunEnvironment::exact(&request.submission.runtime, BTreeMap::new())?;
        self.submit_inner(request, environment, None).await
    }

    /// Trusted bootstrap path for a run whose exact, bounded environment was already selected.
    pub async fn submit_with_exact_environment(
        &self,
        request: RunSubmitParams,
        environment: RunEnvironment,
    ) -> Result<CloudRunReceipt, NativeV2CloudError> {
        self.submit_inner(request, environment, None).await
    }

    /// Trusted target bootstrap with a checkout/delivery credential kept outside provider
    /// environment selection and durable run state.
    pub async fn submit_with_exact_environment_and_github_token(
        &self,
        request: RunSubmitParams,
        environment: RunEnvironment,
        github_token: Option<String>,
    ) -> Result<CloudRunReceipt, NativeV2CloudError> {
        self.submit_inner(request, environment, github_token).await
    }

    async fn submit_inner(
        &self,
        request: RunSubmitParams,
        environment: RunEnvironment,
        github_token: Option<String>,
    ) -> Result<CloudRunReceipt, NativeV2CloudError> {
        let _turn = self.submission_turn.lock().await;
        let RunSubmitParams { run_id, submission } = request;
        let digest = submission_digest(&submission)?;
        let submission_key = submission.submission_key.clone();
        let admitted = NativeV2Admission
            .admit_with_policy(submission, self.delivery_policy)
            .await?;
        let environment = environment.for_runtime(&admitted.runtime)?;
        let created = self
            .ledger
            .create_or_get(CreateRun {
                run_id,
                submission_key,
                submission_digest: digest,
                admitted: admitted.clone(),
            })
            .await?;

        match created {
            CreateRunOutcome::Existing(stored) => {
                self.fail_orphaned_runtime(&stored).await?;
                Ok(CloudRunReceipt {
                    run_id: stored.snapshot.run_id,
                    deduped: true,
                })
            }
            CreateRunOutcome::Created(stored) => {
                self.start_created(
                    stored,
                    admitted,
                    RunSecretEnvelope {
                        environment: Arc::new(environment),
                        github_token,
                    },
                )
                .await
            }
        }
    }

    /// Resolves an exact sourceful retry before consulting its replacement secret envelope.
    pub async fn resolve_submission(
        &self,
        submission_key: &openengine_cluster_protocol::IdempotencyKey,
        submission_digest: &Sha256Digest,
    ) -> Result<Option<CloudRunReceipt>, NativeV2CloudError> {
        let _turn = self.submission_turn.lock().await;
        let Some(stored) = self.ledger.get_by_submission_key(submission_key).await? else {
            return Ok(None);
        };
        if stored.submission_digest != *submission_digest {
            return Err(RunLedgerError::SubmissionConflict {
                existing_run_id: stored.snapshot.run_id,
            }
            .into());
        }
        self.fail_orphaned_runtime(&stored).await?;
        Ok(Some(CloudRunReceipt {
            run_id: stored.snapshot.run_id,
            deduped: true,
        }))
    }

    async fn start_created(
        &self,
        stored: StoredRun,
        admitted: AdmittedRun,
        secrets: RunSecretEnvelope,
    ) -> Result<CloudRunReceipt, NativeV2CloudError> {
        let run_id = stored.snapshot.run_id.clone();
        let controller_claim = self.allocator.claim_controller(&run_id).await?;
        let github_token = match source_github_token(&secrets).await {
            Ok(token) => token,
            Err(error) => {
                self.append_unavailable(&run_id, "runtime_unavailable")
                    .await?;
                return Err(error.into());
            }
        };
        let capsule = match self
            .allocator
            .allocate(&run_id, &admitted, github_token.as_deref())
            .await
        {
            Ok(capsule) => capsule,
            Err(error) => {
                self.allocator
                    .destroy_or_confirm_absent(&run_id, RunRuntimeExit::RuntimeLost)
                    .await
                    .map_err(|_| {
                        NativeV2SupervisorError::RuntimeCleanup(RuntimeCleanupUnavailable)
                    })?;
                self.append_unavailable(&run_id, error.failure_code())
                    .await?;
                return Err(error.into());
            }
        };
        self.start_allocated(AllocatedRunStart {
            stored,
            environment: secrets.environment,
            controller_claim,
            capsule,
        })
        .await
    }

    async fn start_allocated(
        &self,
        start: AllocatedRunStart,
    ) -> Result<CloudRunReceipt, NativeV2CloudError> {
        let AllocatedRunStart {
            stored,
            environment,
            controller_claim,
            capsule,
        } = start;
        let run_id = stored.snapshot.run_id.clone();
        self.observability.track_runtime(&stored.snapshot)?;
        let AllocatedCapsule {
            runner,
            loss,
            cleanup,
        } = capsule;
        let engine = PortableRunEngine::start(PortableRunEngineBootstrap {
            run_id: run_id.clone(),
            ledger: self.ledger.clone(),
            environment: environment.as_ref().clone(),
            runtime: PortableRuntime::with_cleanup(runner, cleanup),
            loss,
            controller_claim,
            delivery_policy: self.delivery_policy,
            observability: self.observability.clone(),
            operator_diagnostics: self.operator_diagnostics.clone(),
        });
        self.runtimes
            .lock()
            .await
            .insert(run_id.clone(), RuntimeSlot::Running(engine.clone()));
        self.remove_finished_runtime(run_id.clone(), engine);
        Ok(CloudRunReceipt {
            run_id,
            deduped: false,
        })
    }

    pub async fn resume(
        &self,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, NativeV2CloudError> {
        let _turn = self.submission_turn.lock().await;
        let RunResumeParams {
            run_id,
            successor_run_id,
            connections,
            connection_resolver,
            github_token,
        } = params;
        let source = self.recoverable_resume_source(&run_id).await?;
        let (submission_key, submission_digest) =
            retained_submission_identity(&run_id, &successor_run_id)?;
        let admitted = source.admitted.clone();
        let secrets = resume_secret_envelope(ResumeSecretRequest {
            admitted: &admitted,
            successor_run_id: &successor_run_id,
            connections,
            connection_resolver,
            github_token,
        })?;
        let github_token = source_github_token(&secrets).await?;
        let controller_claim = self.allocator.claim_controller(&successor_run_id).await?;
        let stored = self
            .create_resume_successor(CreateRun {
                run_id: successor_run_id.clone(),
                submission_key,
                submission_digest,
                admitted: admitted.clone(),
            })
            .await?;
        let capsule = self
            .allocate_retained_capsule(RetainedAllocationRequest {
                source_run_id: &run_id,
                run_id: &successor_run_id,
                admitted: &admitted,
                github_token: github_token.as_deref(),
            })
            .await?;
        self.start_allocated(AllocatedRunStart {
            stored,
            environment: secrets.environment,
            controller_claim,
            capsule,
        })
        .await?;
        Ok(RunResumeResult {
            run_id: successor_run_id,
            resumed_from: run_id,
        })
    }

    async fn recoverable_resume_source(
        &self,
        run_id: &RunId,
    ) -> Result<StoredRun, NativeV2CloudError> {
        let source = self
            .ledger
            .get(run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        let failed = matches!(
            source.snapshot.terminal,
            Some(TerminalResult::Failed { .. })
        );
        if !failed || !self.allocator.workspace_recovery(run_id).await.recoverable {
            return Err(CapsuleAllocationUnavailable::Runtime.into());
        }
        Ok(source)
    }

    async fn create_resume_successor(
        &self,
        request: CreateRun,
    ) -> Result<StoredRun, NativeV2CloudError> {
        let created = self.ledger.create_or_get(request).await?;
        match created {
            CreateRunOutcome::Created(stored) => Ok(stored),
            CreateRunOutcome::Existing(_) => Err(RunLedgerError::RunIdConflict.into()),
        }
    }

    async fn allocate_retained_capsule(
        &self,
        request: RetainedAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, NativeV2CloudError> {
        let run_id = request.run_id;
        let allocation = self.allocator.allocate_from_retained(request).await;
        match allocation {
            Ok(capsule) => Ok(capsule),
            Err(RetainedAllocationUnavailable::Settled(error)) => {
                self.append_unavailable(run_id, error.failure_code())
                    .await?;
                Err(error.into())
            }
            Err(RetainedAllocationUnavailable::CleanupUnconfirmed(_)) => {
                Err(NativeV2SupervisorError::RuntimeCleanup(RuntimeCleanupUnavailable).into())
            }
        }
    }

    pub async fn discard_workspace(
        &self,
        params: RunDiscardWorkspaceParams,
    ) -> Result<RunDiscardWorkspaceResult, NativeV2CloudError> {
        if self.ledger.get(&params.run_id).await?.is_none() {
            return Err(RunLedgerError::RunNotFound.into());
        }
        let discarded = self
            .allocator
            .discard_workspace(&params.run_id)
            .await
            .map_err(|_| NativeV2SupervisorError::RuntimeCleanup(RuntimeCleanupUnavailable))?;
        Ok(RunDiscardWorkspaceResult {
            run_id: params.run_id,
            discarded,
        })
    }

    fn remove_finished_runtime(&self, run_id: RunId, engine: Arc<PortableRunEngine>) {
        let runtimes = self.runtimes.clone();
        tokio::spawn(async move {
            if engine.wait_removable().await {
                runtimes.lock().await.remove(&run_id);
            }
        });
    }

    async fn fail_orphaned_runtime(&self, stored: &StoredRun) -> Result<(), NativeV2CloudError> {
        let _turn = self.reconstructed_turn.lock().await;
        let stored = self
            .ledger
            .get(&stored.snapshot.run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        if stored.snapshot.terminal.is_some()
            || self
                .runtimes
                .lock()
                .await
                .contains_key(&stored.snapshot.run_id)
        {
            return Ok(());
        }
        let _claim = self
            .allocator
            .claim_controller(&stored.snapshot.run_id)
            .await?;
        self.allocator
            .destroy_or_confirm_absent(&stored.snapshot.run_id, RunRuntimeExit::RuntimeLost)
            .await
            .map_err(|_| NativeV2SupervisorError::RuntimeCleanup(RuntimeCleanupUnavailable))?;
        append_runtime_lost(self.ledger.as_ref(), &stored).await?;
        Ok(())
    }

    async fn append_unavailable(
        &self,
        run_id: &RunId,
        code: &str,
    ) -> Result<(), NativeV2CloudError> {
        let stored = self
            .ledger
            .get(run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        append_terminal_failure(self.ledger.as_ref(), &stored, code).await?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<RunSummary>, NativeV2CloudError> {
        Ok(self.ledger.list().await?)
    }

    pub async fn status(
        &self,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, NativeV2CloudError> {
        let mut result = self.observability.status(params).await?;
        let failed = matches!(
            result.status,
            RunStatus::Finished {
                terminal_result: TerminalResult::Failed { .. },
                ..
            }
        );
        if failed {
            result.workspace_recovery = self.allocator.workspace_recovery(&result.run_id).await;
        }
        if failed && result.workspace_recovery.recoverable {
            if let Some(stored) = self.ledger.get(&result.run_id).await? {
                result.workspace_recovery.connection_requirements = stored
                    .admitted
                    .runtime
                    .connection_requirements()
                    .into_iter()
                    .map(|(key, fields)| (key, fields.into_iter().collect()))
                    .collect();
            }
        }
        Ok(result)
    }

    /// Shares observation with a host UI without granting runtime control or creating a controller.
    #[cfg(feature = "ui")]
    #[must_use]
    pub fn observations(&self) -> NativeV2Observability {
        self.observability.clone()
    }

    pub async fn watch(
        &self,
        params: RunWatchParams,
    ) -> Result<(RunWatchResult, RunWatchSubscription), NativeV2CloudError> {
        Ok(self.observability.watch(params).await?)
    }

    pub async fn logs(
        &self,
        params: RunLogsParams,
    ) -> Result<(RunLogsResult, RunLogsSubscription), NativeV2CloudError> {
        Ok(self.observability.logs(params).await?)
    }

    pub async fn attach(
        &self,
        params: RunAttachParams,
    ) -> Result<(RunAttachResult, RunAttachSubscription), NativeV2CloudError> {
        Ok(self.observability.attach(params).await?)
    }

    pub async fn force(
        &self,
        params: RunForceParams,
    ) -> Result<RunForceResult, NativeV2CloudError> {
        match self.prepare_force(&params.run_id).await? {
            ForceTarget::Terminal => {}
            ForceTarget::Running(supervisor) => {
                self.force_running(&params.run_id, &supervisor).await?;
            }
            ForceTarget::Reconstructed => {
                self.force_reconstructed(&params.run_id).await?;
            }
        }
        self.force_result(&params.run_id).await
    }

    async fn prepare_force(&self, run_id: &RunId) -> Result<ForceTarget, NativeV2CloudError> {
        // An existing runtime owns cancellation independently of submission and storage.
        if let Some(RuntimeSlot::Running(engine)) = self.runtimes.lock().await.get(run_id).cloned()
        {
            return Ok(ForceTarget::Running(engine));
        }
        // Without a live owner, wait out durable creation/allocation before reconstructing.
        let _turn = self.submission_turn.lock().await;
        if let Some(RuntimeSlot::Running(engine)) = self.runtimes.lock().await.get(run_id).cloned()
        {
            return Ok(ForceTarget::Running(engine));
        }
        let stored = self
            .ledger
            .get(run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        if stored.snapshot.terminal.is_some() {
            return Ok(ForceTarget::Terminal);
        }
        self.ledger.request_force_stop(run_id).await?;
        Ok(ForceTarget::Reconstructed)
    }

    async fn force_running(
        &self,
        run_id: &RunId,
        engine: &PortableRunEngine,
    ) -> Result<(), NativeV2CloudError> {
        // `drive` is internally serialized. Usually this waits behind the live driving turn; if
        // that turn stopped on cleanup error, this is the one retry that can finish cleanup.
        engine.force_stop().await?;
        self.runtimes.lock().await.remove(run_id);
        Ok(())
    }

    async fn force_reconstructed(&self, run_id: &RunId) -> Result<(), NativeV2CloudError> {
        let _turn = self.reconstructed_turn.lock().await;
        let stored = self
            .ledger
            .get(run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        if stored.snapshot.terminal.is_some() {
            return Ok(());
        }
        let _claim = self.allocator.claim_controller(run_id).await?;
        self.allocator
            .destroy_or_confirm_absent(run_id, RunRuntimeExit::ForceStopped)
            .await
            .map_err(|_| NativeV2SupervisorError::RuntimeCleanup(RuntimeCleanupUnavailable))?;
        let stored = self
            .ledger
            .get(run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        append_terminal_failure(self.ledger.as_ref(), &stored, "force_stopped").await?;
        Ok(())
    }

    async fn force_result(&self, run_id: &RunId) -> Result<RunForceResult, NativeV2CloudError> {
        let RunStatusResult {
            run_id,
            title,
            source,
            size,
            at_cursor,
            status,
            workspace_recovery,
        } = self
            .observability
            .status(RunStatusParams {
                run_id: run_id.clone(),
            })
            .await?;
        Ok(RunForceResult {
            run_id,
            title,
            source,
            size,
            at_cursor,
            status,
            workspace_recovery,
        })
    }
}

fn retained_submission_identity(
    source_run_id: &RunId,
    successor_run_id: &RunId,
) -> Result<(openengine_cluster_protocol::IdempotencyKey, Sha256Digest), NativeV2CloudError> {
    let submission_key = openengine_cluster_protocol::IdempotencyKey::new(format!(
        "resume-{}",
        successor_run_id.as_str()
    ))
    .map_err(|_| NativeV2CloudError::SubmissionIdentity)?;
    let submission_digest = Sha256Digest::new(format!(
        "{:x}",
        Sha256::digest(format!(
            "{}\0{}",
            source_run_id.as_str(),
            successor_run_id.as_str()
        ))
    ))
    .map_err(|_| NativeV2CloudError::SubmissionIdentity)?;
    Ok((submission_key, submission_digest))
}

fn resume_secret_envelope(
    request: ResumeSecretRequest<'_>,
) -> Result<RunSecretEnvelope, NativeV2CloudError> {
    let ResumeSecretRequest {
        admitted,
        successor_run_id,
        connections,
        connection_resolver,
        github_token,
    } = request;
    let environment = match connection_resolver {
        Some(wire) => RunEnvironment::with_resolver(
            &admitted.runtime,
            connections,
            crate::native_v2_hosting::build_connection_resolver(successor_run_id.clone(), wire)
                .map_err(|_| NativeV2CloudError::ResumeCredentials)?,
        ),
        None => RunEnvironment::exact(&admitted.runtime, connections),
    }?;
    Ok(RunSecretEnvelope {
        environment: Arc::new(environment),
        github_token,
    })
}

mod backend;
mod source;
use backend::{append_runtime_lost, append_terminal_failure};
pub(crate) use backend::submission_digest;
use source::source_github_token;
