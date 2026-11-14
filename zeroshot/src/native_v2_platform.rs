//! Transport-neutral platform controller for one native-v2 run per disposable capsule.
//!
//! Admission, durable graph truth, supervision, observation, environment selection, and terminal
//! policy remain controller-owned. The allocator supplies exactly one opaque node runner, one
//! liveness signal, and one result-bearing cleanup authority for the run's capsule/workspace.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ClusterStatus, GetParams, GetResult, GraphProfile, GraphProfileSet, InitializeParams,
    InitializeResult, RunAttachEventNotification, RunAttachParams, RunAttachResult, RunForceParams,
    RunForceResult, RunId, RunListParams, RunListResult, RunDiscardWorkspaceParams,
    RunDiscardWorkspaceResult, RunLogEventNotification, RunLogsParams, RunLogsResult,
    RunResumeParams, RunResumeResult, RunStatus, RunStatusParams, RunStatusResult, RunSubmitParams,
    RunSubmitResult, RunWatchEventNotification, RunWatchParams, RunWatchResult, ServerCapabilities,
    Sha256Digest, SubscriptionCloseReason, TerminalResult, WorkerErrorCode, WorkerOutcome, GONE,
    GRAPH_INVALID, IDEMPOTENCY_REUSE, INTERNAL_ERROR_CODE, NOT_FOUND, SCHEMA_VIOLATION,
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
use crate::native_v2_contract::{AdmittedRun, RunSubmission};
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
#[path = "native_v2_platform/tests.rs"]
mod tests;

mod contracts;
mod preparation;
use preparation::{PreparationTask, PreparationStart, ResumePreparation};
pub use contracts::{
    AllocatedCapsule, CapsuleAllocationRequest, CapsuleAllocationUnavailable, CapsuleAllocator,
    CapsuleCleanup, CapsulePreparation, PreparationProgress, CapsuleCleanupUnavailable,
    CapsuleDestroyed, PlatformRunReceipt, ControllerClaimUnavailable, ExclusiveControllerClaim,
    RetainedAllocationRequest, RetainedAllocationUnavailable, RunSourceCredentials,
};
pub use crate::native_v2_supervisor::RunEnvironment;

/// Compatibility name for the protocol-owned submission while downstream callers migrate.
pub type PlatformRunSubmission = RunSubmission;

#[derive(Debug, Error)]
pub enum NativeV2PlatformError {
    #[error("{0}")]
    InvalidRequest(String),
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
    Checkpoint(#[from] crate::native_v2_supervisor::checkpoints::CheckpointError),
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
pub struct NativeV2PlatformController {
    ledger: Arc<dyn RunLedger>,
    allocator: Arc<dyn CapsuleAllocator>,
    observability: NativeV2Observability,
    runtimes: Arc<Mutex<BTreeMap<RunId, RuntimeSlot>>>,
    submission_turn: Arc<Mutex<()>>,
    control_turns: Arc<Mutex<BTreeMap<RunId, Weak<Mutex<()>>>>>,
    reconstructed_turn: Arc<Mutex<()>>,
    delivery_policy: DeliveryPolicy,
    harnesses: Arc<crate::harnesses::HarnessRegistry>,
    run_sizes: Option<Arc<openengine_cluster_protocol::TargetRunSizesDiscovery>>,
    operator_diagnostics: Arc<crate::native_v2_target_authority::OperatorDiagnosticStore>,
}

#[derive(Clone)]
enum RuntimeSlot {
    Preparing(Arc<PreparationTask>),
    Running(Arc<PortableRunEngine>),
}

enum ForceTarget {
    Preparing(Arc<PreparationTask>),
    Terminal,
    Running(Arc<PortableRunEngine>),
    Reconstructed,
}

struct RunSecretEnvelope {
    environment: Arc<RunEnvironment>,
    source_credentials: RunSourceCredentials,
}

struct ResumeSecretRequest<'a> {
    admitted: &'a AdmittedRun,
    successor_run_id: &'a RunId,
    connections: openengine_cluster_protocol::RunConnectionValues,
    connection_resolver: Option<openengine_cluster_protocol::TargetConnectionResolver>,
    github_token: Option<String>,
    git_credentials: Option<openengine_cluster_protocol::GitTransportCredentials>,
    commit_identity: Option<openengine_cluster_protocol::CommitIdentity>,
    commit_co_author: Option<openengine_cluster_protocol::CommitIdentity>,
}

struct AllocatedRunStart {
    stored: StoredRun,
    environment: Arc<RunEnvironment>,
    controller_claim: Arc<dyn ExclusiveControllerClaim>,
    capsule: AllocatedCapsule,
}

impl NativeV2PlatformController {
    /// Claims exclusive target authority, reconciles durable nonterminal runs, then constructs
    /// the target adapter. Each submitted run carries its own immutable runtime plan.
    pub async fn new(
        ledger: Arc<dyn RunLedger>,
        allocator: Arc<dyn CapsuleAllocator>,
    ) -> Result<Self, NativeV2PlatformError> {
        Self::new_with_delivery_policy(ledger, allocator, DeliveryPolicy::Required).await
    }

    pub async fn new_with_delivery_policy(
        ledger: Arc<dyn RunLedger>,
        allocator: Arc<dyn CapsuleAllocator>,
        delivery_policy: DeliveryPolicy,
    ) -> Result<Self, NativeV2PlatformError> {
        let controller = Self {
            observability: NativeV2Observability::new(ledger.clone())
                .with_workspace_recovery(allocator.clone()),
            ledger,
            allocator,
            runtimes: Arc::new(Mutex::new(BTreeMap::new())),
            submission_turn: Arc::new(Mutex::new(())),
            control_turns: Arc::default(),
            reconstructed_turn: Arc::new(Mutex::new(())),
            delivery_policy,
            harnesses: Arc::new(crate::harnesses::HarnessRegistry::core()),
            run_sizes: None,
            operator_diagnostics: Arc::default(),
        };
        controller.reconcile_persisted_runs().await?;
        Ok(controller)
    }

    fn require_wait_support(
        &self,
        runtime: &crate::native_v2_contract::RuntimePlan,
    ) -> Result<(), NativeV2AdmissionError> {
        if self.allocator.suspension_policy().is_none()
            && let Some((node, _)) = runtime.nodes().iter().find(|(_, binding)| {
                matches!(
                    binding,
                    openengine_cluster_protocol::NodeRuntimeBinding::Request {}
                )
            })
        {
            return Err(NativeV2AdmissionError::InvalidRuntimeBinding {
                node: node.clone(),
                detail: "host does not enable openengine.request-wait/v1".to_owned(),
            });
        }
        Ok(())
    }

    fn validate_harnesses(
        &self,
        runtime: &crate::native_v2_contract::RuntimePlan,
    ) -> Result<(), NativeV2AdmissionError> {
        self.require_wait_support(runtime)?;
        self.harnesses
            .validate_runtime(runtime)
            .map_err(|error| NativeV2AdmissionError::InvalidRuntime(error.to_string()))
    }

    /// Selects the host's enabled implementations before serving submissions.
    /// The allocator must construct adapters from this same registry.
    #[must_use]
    pub fn with_harness_registry(mut self, harnesses: crate::harnesses::HarnessRegistry) -> Self {
        self.harnesses = Arc::new(harnesses);
        self
    }

    /// Installs this host's size choices without constraining the portable protocol.
    #[must_use]
    pub fn with_run_sizes(
        mut self,
        run_sizes: openengine_cluster_protocol::TargetRunSizesDiscovery,
    ) -> Self {
        self.run_sizes = Some(Arc::new(run_sizes));
        self
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
    async fn reconcile_persisted_runs(&self) -> Result<(), NativeV2PlatformError> {
        for summary in self.ledger.list().await? {
            let stored = self
                .ledger
                .get(&summary.run_id)
                .await?
                .ok_or(RunLedgerError::RunNotFound)?;
            if stored.snapshot.terminal.is_some()
                || stored.snapshot.phase == crate::v2_run_ledger::RunPhase::Suspended
            {
                continue;
            }
            let _claim = self.allocator.claim_controller(&summary.run_id).await?;
            self.finish_lost_runtime(&stored).await?;
        }
        Ok(())
    }

    async fn finish_lost_runtime(&self, stored: &StoredRun) -> Result<(), NativeV2PlatformError> {
        let run_id = &stored.snapshot.run_id;
        // Quarantine interrupted work and retain checkpoints until terminal publication succeeds.
        self.allocator
            .destroy_or_confirm_absent(run_id, RunRuntimeExit::RuntimeLost)
            .await
            .map_err(|_| NativeV2SupervisorError::RuntimeCleanup(RuntimeCleanupUnavailable))?;
        append_interrupted(
            self.ledger.as_ref(),
            stored,
            "runtime_lost",
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
        )
        .await?;
        if stored
            .snapshot
            .primary_graph
            .as_ref()
            .is_some_and(|primary| matches!(primary.result, TerminalResult::Succeeded { .. }))
        {
            // A confirmed, successful lineage no longer needs the retained target workspace.
            // Garbage collection stays best effort and strictly after durable terminal truth.
            let _ = self.allocator.discard_workspace(run_id).await;
        }
        Ok(())
    }

    /// Admits before every durable or allocation effect. Run identity is assigned by the host and
    /// exact resubmissions retain that identity without allocating a replacement runtime.
    pub async fn submit(
        &self,
        request: RunSubmitParams,
    ) -> Result<PlatformRunReceipt, NativeV2PlatformError> {
        self.require_wait_support(&request.submission.runtime)?;
        let environment =
            RunEnvironment::exact_for_submission(&request.submission, BTreeMap::new())?;
        self.submit_inner(request, environment, RunSourceCredentials::default())
            .await
    }

    /// Trusted bootstrap path for a run whose exact, bounded environment was already selected.
    pub async fn submit_with_exact_environment(
        &self,
        request: RunSubmitParams,
        environment: RunEnvironment,
    ) -> Result<PlatformRunReceipt, NativeV2PlatformError> {
        self.submit_inner(request, environment, RunSourceCredentials::default())
            .await
    }

    /// Trusted target bootstrap with a checkout/delivery credential kept outside provider
    /// environment selection and durable run state.
    pub async fn submit_with_exact_environment_and_github_token(
        &self,
        request: RunSubmitParams,
        environment: RunEnvironment,
        github_token: Option<String>,
    ) -> Result<PlatformRunReceipt, NativeV2PlatformError> {
        self.submit_inner(
            request,
            environment,
            RunSourceCredentials {
                github_token,
                git_credentials: None,
                commit_identity: None,
                commit_co_author: None,
            },
        )
        .await
    }

    /// Keeps generic Git and optional forge authority private through checkout and allocation.
    pub async fn submit_with_exact_environment_and_source_credentials(
        &self,
        request: RunSubmitParams,
        environment: RunEnvironment,
        credentials: RunSourceCredentials,
    ) -> Result<PlatformRunReceipt, NativeV2PlatformError> {
        self.submit_inner(request, environment, credentials).await
    }

    async fn submit_inner(
        &self,
        request: RunSubmitParams,
        environment: RunEnvironment,
        source_credentials: RunSourceCredentials,
    ) -> Result<PlatformRunReceipt, NativeV2PlatformError> {
        let _turn = self.submission_turn.lock().await;
        let RunSubmitParams { run_id, submission } = request;
        let harnesses = self.harnesses.as_ref();
        let digest = submission_digest(&submission)?;
        let submission_key = submission.submission_key.clone();
        if let Some(receipt) = self.existing_submission(&submission_key, &digest).await? {
            return Ok(receipt);
        }
        if !self.allocator.supports_request_ask() {
            if let Some(node) = find_request_node(&submission.graph.root) {
                return Err(NativeV2AdmissionError::InvalidRuntimeBinding {
                    node: node.clone(),
                    detail: "host does not enable openengine.request-ask/v1".to_owned(),
                }
                .into());
            }
        }
        self.validate_harnesses(&submission.runtime)?;
        let mut admission = crate::native_v2_admission::NativeV2AdmissionContext::new(
            harnesses,
            self.delivery_policy,
        );
        if let Some(catalog) = self.run_sizes.as_deref() {
            admission = admission.with_run_sizes(catalog);
        }
        let admitted = NativeV2Admission
            .admit_with_context(submission, admission)
            .await?;
        let environment = environment.for_admitted(&admitted)?;
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
                Ok(PlatformRunReceipt {
                    run_id: stored.snapshot.run_id,
                    deduped: true,
                })
            }
            CreateRunOutcome::Created(stored) => {
                self.start_created(
                    stored,
                    RunSecretEnvelope {
                        environment: Arc::new(environment),
                        source_credentials,
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
    ) -> Result<Option<PlatformRunReceipt>, NativeV2PlatformError> {
        let _turn = self.submission_turn.lock().await;
        self.existing_submission(submission_key, submission_digest)
            .await
    }

    async fn existing_submission(
        &self,
        submission_key: &openengine_cluster_protocol::IdempotencyKey,
        submission_digest: &Sha256Digest,
    ) -> Result<Option<PlatformRunReceipt>, NativeV2PlatformError> {
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
        Ok(Some(PlatformRunReceipt {
            run_id: stored.snapshot.run_id,
            deduped: true,
        }))
    }

    async fn start_created(
        &self,
        stored: StoredRun,
        secrets: RunSecretEnvelope,
    ) -> Result<PlatformRunReceipt, NativeV2PlatformError> {
        let run_id = stored.snapshot.run_id.clone();
        let controller_claim = self.allocator.claim_controller(&run_id).await?;
        self.start_preparation(PreparationStart {
            stored,
            secrets,
            controller_claim,
            resume: None,
        })
        .await?;
        Ok(PlatformRunReceipt {
            run_id,
            deduped: false,
        })
    }

    async fn start_allocated(
        &self,
        start: AllocatedRunStart,
    ) -> Result<PlatformRunReceipt, NativeV2PlatformError> {
        let AllocatedRunStart {
            mut stored,
            environment,
            controller_claim,
            capsule,
        } = start;
        let run_id = stored.snapshot.run_id.clone();
        let AllocatedCapsule {
            vector_seed,
            checkpoints,
            execution_seed,
            runner,
            loss,
            cleanup,
        } = capsule;
        if let Err(error) = self
            .initialize_allocated(&mut stored, execution_seed, vector_seed)
            .await
        {
            // Runtime-loss cleanup preserves the restored workspace and its recovery lineage.
            cleanup
                .destroy_or_confirm_absent(RunRuntimeExit::RuntimeLost)
                .await
                .map_err(|_| NativeV2SupervisorError::RuntimeCleanup(RuntimeCleanupUnavailable))?;
            self.append_unavailable(&run_id, "runtime_unavailable")
                .await?;
            return Err(error);
        }
        let mut runtime = PortableRuntime::with_cleanup(runner, cleanup);
        runtime.checkpoints = checkpoints;
        let engine = PortableRunEngine::start(PortableRunEngineBootstrap {
            wait_policy: self.allocator.suspension_policy().unwrap_or_default(),
            run_id: run_id.clone(),
            ledger: self.ledger.clone(),
            environment: environment.as_ref().clone(),
            runtime,
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
        Ok(PlatformRunReceipt {
            run_id,
            deduped: false,
        })
    }

    async fn initialize_allocated(
        &self,
        stored: &mut StoredRun,
        execution_seed: Vec<crate::full_v1_reducer::DurableExecution>,
        vector_seed: Vec<openengine_cluster_protocol::RunVector>,
    ) -> Result<(), NativeV2PlatformError> {
        for vector in vector_seed {
            stored.snapshot = self
                .ledger
                .append(&stored.snapshot.run_id, vec![RunEvent::Vector { vector }])
                .await?
                .snapshot;
        }
        if !execution_seed.is_empty() {
            let events = execution_seed
                .into_iter()
                .map(|execution| RunEvent::PriorExecution { execution })
                .collect();
            stored.snapshot = self
                .ledger
                .append(&stored.snapshot.run_id, events)
                .await?
                .snapshot;
        }
        self.observability.track_runtime(&stored.snapshot)?;
        Ok(())
    }

    pub async fn resume(
        &self,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, NativeV2PlatformError> {
        let _turn = self.submission_turn.lock().await;
        let RunResumeParams {
            from,
            run_id,
            successor_run_id,
            connections,
            connection_resolver,
            github_token,
            git_credentials,
            commit_identity,
            commit_co_author,
        } = params;
        if matches!(
            from,
            Some(
                openengine_cluster_protocol::RunResumeFrom::Answer { .. }
                    | openengine_cluster_protocol::RunResumeFrom::Expiry {}
            )
        ) && !self.allocator.supports_request_ask()
        {
            return Err(NativeV2PlatformError::InvalidRequest(
                "host does not enable openengine.request-ask/v1".to_owned(),
            ));
        }
        // Refuse a live source before taking its control turn: cancellation must not wait for
        // a resume's ledger reads. Recheck eligibility once ownership is serialized with force.
        self.recoverable_resume_source(&run_id).await?;
        let _control = self.lock_run_control(&run_id).await;
        let source = self.recoverable_resume_source(&run_id).await?;
        let request_resolution = if let Some(suspension) = &source.snapshot.suspension {
            if matches!(
                suspension,
                openengine_cluster_protocol::RunSuspension::Request { .. }
            ) || matches!(
                from,
                Some(
                    openengine_cluster_protocol::RunResumeFrom::Answer { .. }
                        | openengine_cluster_protocol::RunResumeFrom::Expiry {}
                )
            ) {
                Some(
                    crate::native_v2_request::resolve(
                        suspension,
                        from.as_ref(),
                        (self
                            .allocator
                            .suspension_policy()
                            .unwrap_or_default()
                            .wall_clock)(),
                    )
                    .map_err(|message| {
                        NativeV2PlatformError::InvalidRequest(
                            message.replace("RUN_ID", run_id.as_str()),
                        )
                    })?,
                )
            } else {
                None
            }
        } else if matches!(
            from,
            Some(
                openengine_cluster_protocol::RunResumeFrom::Answer { .. }
                    | openengine_cluster_protocol::RunResumeFrom::Expiry {}
            )
        ) {
            return Err(NativeV2PlatformError::InvalidRequest(
                "answer requires a suspended run".to_owned(),
            ));
        } else {
            None
        };
        if let Some(resolution) = &request_resolution {
            crate::native_v2_request::validate_resolution(&source.snapshot, resolution)
                .map_err(NativeV2PlatformError::InvalidRequest)?;
        }
        let expired = matches!(
            request_resolution,
            Some(openengine_cluster_protocol::RequestResolution::Expiry {})
        );
        if let Some(suspension) = &source.snapshot.suspension {
            // A settled attempt must release its owner before the successor can claim storage.
            self.wait_runtime_settled(&run_id).await;
            self.runtimes.lock().await.remove(&run_id);
            if from.is_some() && request_resolution.is_none() {
                return Err(NativeV2PlatformError::InvalidRequest(
                    "suspended runs accept only default resume".to_owned(),
                ));
            }
            if let Some(due) = suspension.due_at()
                && (self
                    .allocator
                    .suspension_policy()
                    .unwrap_or_default()
                    .wall_clock)()
                    < due
            {
                return Err(NativeV2PlatformError::InvalidRequest(format!(
                    "wait is due at {}",
                    due.get()
                )));
            }
        }
        // Reconcile before claiming/moving the workspace or creating a successor. A hard exit
        // may leave acknowledged ledger receipts newer than the last workspace boundary.
        if let Some(state) = source.snapshot.delivery_checkpoint()? {
            self.allocator
                .retain_delivery_state(&run_id, &state)
                .await?;
        }
        let (submission_key, submission_digest) =
            retained_submission_identity(&run_id, &successor_run_id)?;
        let mut admitted = source.admitted.clone();
        if let Some(catalog) = self.run_sizes.as_deref() {
            crate::native_v2_admission::resolve_runtime_size(&mut admitted.runtime, catalog)?;
        }
        self.validate_harnesses(&admitted.runtime)?;
        let secrets = resume_secret_envelope(ResumeSecretRequest {
            admitted: &admitted,
            successor_run_id: &successor_run_id,
            connections,
            connection_resolver,
            github_token,
            git_credentials,
            commit_identity,
            commit_co_author,
        })?;
        let controller_claim = self.allocator.claim_controller(&successor_run_id).await?;
        let stored = self
            .create_resume_successor(CreateRun {
                run_id: successor_run_id.clone(),
                submission_key,
                submission_digest,
                admitted: admitted.clone(),
            })
            .await?;
        self.start_preparation(PreparationStart {
            stored,
            secrets,
            controller_claim,
            resume: Some(ResumePreparation {
                source_run_id: run_id.clone(),
                request_resolution,
                selection: if source.snapshot.phase == crate::v2_run_ledger::RunPhase::Suspended {
                    crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection::Continue
                } else {
                    crate::native_v2_supervisor::checkpoints::restore_selection(from.as_ref())
                },
            }),
        })
        .await?;
        if expired {
            return Err(NativeV2PlatformError::InvalidRequest(format!(
                "request expired; successor {}",
                successor_run_id.as_str()
            )));
        }
        Ok(RunResumeResult {
            run_id: successor_run_id,
            resumed_from: run_id,
        })
    }

    async fn recoverable_resume_source(
        &self,
        run_id: &RunId,
    ) -> Result<StoredRun, NativeV2PlatformError> {
        let source = self
            .ledger
            .get(run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        let failed = matches!(
            source.snapshot.terminal,
            Some(TerminalResult::Failed { .. })
        );
        let recovery = self.allocator.workspace_recovery(run_id).await;
        if let Some(successor) = &recovery.successor_run_id {
            return Err(NativeV2PlatformError::InvalidRequest(format!(
                "retained workspace was already claimed by {}",
                successor.as_str()
            )));
        }
        if (failed && source.snapshot.suspension.is_some())
            || !(failed || source.snapshot.phase == crate::v2_run_ledger::RunPhase::Suspended)
            || !recovery.recoverable
        {
            return Err(CapsuleAllocationUnavailable::Runtime.into());
        }
        Ok(source)
    }

    async fn create_resume_successor(
        &self,
        request: CreateRun,
    ) -> Result<StoredRun, NativeV2PlatformError> {
        let created = self.ledger.create_or_get(request).await?;
        match created {
            CreateRunOutcome::Created(stored) => Ok(stored),
            CreateRunOutcome::Existing(_) => Err(RunLedgerError::RunIdConflict.into()),
        }
    }

    pub async fn suspend(
        &self,
        params: openengine_cluster_protocol::RunSuspendParams,
    ) -> Result<openengine_cluster_protocol::RunSuspendResult, NativeV2PlatformError> {
        let engine = match self.runtimes.lock().await.get(&params.run_id) {
            Some(RuntimeSlot::Running(engine)) => engine.clone(),
            _ => {
                return Err(NativeV2PlatformError::InvalidRequest(
                    "suspend requires a running run".to_owned(),
                ));
            }
        };
        if self.allocator.suspension_policy().is_none() {
            return Err(NativeV2PlatformError::InvalidRequest(
                "host does not enable openengine.run-suspension/v1".to_owned(),
            ));
        }
        engine.suspend().await?;
        self.status(params).await
    }

    pub async fn vectors(
        &self,
        params: openengine_cluster_protocol::RunVectorsParams,
    ) -> Result<openengine_cluster_protocol::RunVectorsResult, NativeV2PlatformError> {
        Ok(crate::v2_run_ledger::vectors::list(self.ledger.as_ref(), params).await?)
    }
    pub async fn vector(
        &self,
        params: openengine_cluster_protocol::RunVectorParams,
    ) -> Result<openengine_cluster_protocol::RunVectorResult, NativeV2PlatformError> {
        Ok(crate::v2_run_ledger::vectors::show(self.ledger.as_ref(), params).await?)
    }

    pub async fn checkpoints(
        &self,
        params: openengine_cluster_protocol::RunCheckpointsParams,
    ) -> Result<openengine_cluster_protocol::RunCheckpointsResult, NativeV2PlatformError> {
        if self.ledger.get(&params.run_id).await?.is_none() {
            return Err(RunLedgerError::RunNotFound.into());
        }
        Ok(self.allocator.checkpoints(params).await?)
    }

    pub async fn discard_workspace(
        &self,
        params: RunDiscardWorkspaceParams,
    ) -> Result<RunDiscardWorkspaceResult, NativeV2PlatformError> {
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

    /// Waits until the run's live runtime, if any, has settled its observed outcome, including
    /// the terminal append attempt that follows a published runtime failure. A preparation hands
    /// off to its engine before it finishes, so the engine that replaced it is awaited too.
    pub(crate) async fn wait_runtime_settled(&self, run_id: &RunId) {
        loop {
            // Read the slot and its preparation state under one lock: a finished preparation
            // still in its slot can no longer install an engine.
            let runtime = match self.runtimes.lock().await.get(run_id) {
                Some(RuntimeSlot::Preparing(task)) if task.is_finished() => None,
                runtime => runtime.cloned(),
            };
            match runtime {
                Some(RuntimeSlot::Preparing(task)) => task.wait().await,
                Some(RuntimeSlot::Running(engine)) => {
                    engine.wait_removable().await;
                    return;
                }
                None => return,
            }
        }
    }

    fn remove_finished_runtime(&self, run_id: RunId, engine: Arc<PortableRunEngine>) {
        let runtimes = self.runtimes.clone();
        tokio::spawn(async move {
            if engine.wait_removable().await {
                runtimes.lock().await.remove(&run_id);
            }
        });
    }

    async fn fail_orphaned_runtime(&self, stored: &StoredRun) -> Result<(), NativeV2PlatformError> {
        let _turn = self.reconstructed_turn.lock().await;
        let stored = self
            .ledger
            .get(&stored.snapshot.run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        if stored.snapshot.terminal.is_some()
            || stored.snapshot.phase == crate::v2_run_ledger::RunPhase::Suspended
        {
            return Ok(());
        }
        let runtime = self
            .runtimes
            .lock()
            .await
            .get(&stored.snapshot.run_id)
            .cloned();
        if let Some(runtime) = runtime {
            return self
                .reconcile_owned_preparation(&stored.snapshot.run_id, runtime)
                .await;
        }
        let _claim = self
            .allocator
            .claim_controller(&stored.snapshot.run_id)
            .await?;
        self.finish_lost_runtime(&stored).await?;
        Ok(())
    }

    async fn append_unavailable(
        &self,
        run_id: &RunId,
        code: &str,
    ) -> Result<(), NativeV2PlatformError> {
        let stored = self
            .ledger
            .get(run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        append_terminal_failure(
            self.ledger.as_ref(),
            &stored,
            code,
            WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
        )
        .await?;
        self.observability.runtime_finished(run_id);
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<RunSummary>, NativeV2PlatformError> {
        Ok(self.ledger.list().await?)
    }

    pub async fn status(
        &self,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, NativeV2PlatformError> {
        let mut result = self.observability.status(params).await?;
        let failed = matches!(
            result.status,
            RunStatus::Finished {
                terminal_result: TerminalResult::Failed { .. },
                ..
            }
        );
        result.workspace_recovery = self.allocator.workspace_recovery(&result.run_id).await;
        let suspended = matches!(result.status, RunStatus::Suspended { .. });
        if !(failed || suspended) {
            result.workspace_recovery.recoverable = false;
        }
        if let RunStatus::Suspended {
            suspension: openengine_cluster_protocol::RunSuspension::Request { .. },
            ..
        } = &result.status
        {
            result.workspace_recovery.recoverable = false;
        }
        if let RunStatus::Suspended { suspension, .. } = &result.status
            && suspension.due_at().is_some_and(|due| {
                (self
                    .allocator
                    .suspension_policy()
                    .unwrap_or_default()
                    .wall_clock)()
                    < due
            })
        {
            result.workspace_recovery.recoverable = false;
        }
        if suspended || (failed && result.workspace_recovery.recoverable) {
            if let Some(stored) = self.ledger.get(&result.run_id).await? {
                result.workspace_recovery.connection_requirements = stored
                    .admitted
                    .connection_requirements()
                    .into_iter()
                    .map(|(key, fields)| (key, fields.into_iter().collect()))
                    .collect();
            }
        }
        Ok(result)
    }

    /// Shares observation with a host UI without granting runtime control or creating a controller.
    #[must_use]
    pub fn observations(&self) -> NativeV2Observability {
        self.observability.clone()
    }

    pub async fn watch(
        &self,
        params: RunWatchParams,
    ) -> Result<(RunWatchResult, RunWatchSubscription), NativeV2PlatformError> {
        Ok(self.observability.watch(params).await?)
    }

    pub async fn logs(
        &self,
        params: RunLogsParams,
    ) -> Result<(RunLogsResult, RunLogsSubscription), NativeV2PlatformError> {
        Ok(self.observability.logs(params).await?)
    }

    pub async fn attach(
        &self,
        params: RunAttachParams,
    ) -> Result<(RunAttachResult, RunAttachSubscription), NativeV2PlatformError> {
        Ok(self.observability.attach(params).await?)
    }

    pub async fn force(
        &self,
        params: RunForceParams,
    ) -> Result<RunForceResult, NativeV2PlatformError> {
        match self.prepare_force(&params.run_id).await? {
            ForceTarget::Terminal => {}
            ForceTarget::Preparing(task) => {
                self.force_preparing(&params.run_id, &task).await?;
            }
            ForceTarget::Running(supervisor) => {
                self.force_running(&params.run_id, &supervisor).await?;
            }
            ForceTarget::Reconstructed => {
                self.force_reconstructed(&params.run_id).await?;
            }
        }
        self.force_result(&params.run_id).await
    }

    async fn live_force_target(&self, run_id: &RunId) -> Option<ForceTarget> {
        self.runtimes
            .lock()
            .await
            .get(run_id)
            .cloned()
            .map(|runtime| match runtime {
                RuntimeSlot::Running(engine) => ForceTarget::Running(engine),
                RuntimeSlot::Preparing(task) => ForceTarget::Preparing(task),
            })
    }

    async fn prepare_force(&self, run_id: &RunId) -> Result<ForceTarget, NativeV2PlatformError> {
        // Capture live ownership before reconstruction; a running engine rechecks successor
        // ownership under its control turn before it can be signalled.
        if let Some(target) = self.live_force_target(run_id).await {
            return Ok(target);
        }
        // Without a live owner, wait out durable creation/allocation before reconstructing.
        let _turn = self.submission_turn.lock().await;
        if let Some(target) = self.live_force_target(run_id).await {
            return Ok(target);
        }
        let stored = self
            .ledger
            .get(run_id)
            .await?
            .ok_or(RunLedgerError::RunNotFound)?;
        if stored.snapshot.terminal.is_some()
            || (stored.snapshot.phase == crate::v2_run_ledger::RunPhase::Suspended
                && self
                    .allocator
                    .workspace_recovery(run_id)
                    .await
                    .successor_run_id
                    .is_some())
        {
            return Ok(ForceTarget::Terminal);
        }
        self.ledger.request_force_stop(run_id).await?;
        Ok(ForceTarget::Reconstructed)
    }

    async fn lock_run_control(&self, run_id: &RunId) -> tokio::sync::OwnedMutexGuard<()> {
        let turn = {
            let mut turns = self.control_turns.lock().await;
            turns.retain(|_, turn| turn.strong_count() > 0);
            if let Some(turn) = turns.get(run_id).and_then(Weak::upgrade) {
                turn
            } else {
                let turn = Arc::new(Mutex::new(()));
                turns.insert(run_id.clone(), Arc::downgrade(&turn));
                turn
            }
        };
        turn.lock_owned().await
    }

    async fn force_running(
        &self,
        run_id: &RunId,
        engine: &PortableRunEngine,
    ) -> Result<(), NativeV2PlatformError> {
        // A captured engine may already have published suspension. Serialize its force request
        // with continuation ownership, including persistence, so it cannot stop a claimed source.
        let _control = self.lock_run_control(run_id).await;
        if self
            .allocator
            .workspace_recovery(run_id)
            .await
            .successor_run_id
            .is_some()
        {
            return Ok(());
        }
        // `drive` is internally serialized. Usually this waits behind the live driving turn; if
        // that turn stopped on cleanup error, this is the one retry that can finish cleanup.
        engine.force_stop().await?;
        self.runtimes.lock().await.remove(run_id);
        Ok(())
    }

    async fn force_reconstructed(&self, run_id: &RunId) -> Result<(), NativeV2PlatformError> {
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
        append_interrupted(
            self.ledger.as_ref(),
            &stored,
            "force_stopped",
            WorkerOutcome::stopped(),
        )
        .await?;
        Ok(())
    }

    async fn force_result(&self, run_id: &RunId) -> Result<RunForceResult, NativeV2PlatformError> {
        let RunStatusResult {
            run_id,
            title,
            source,
            size,
            at_cursor,
            status,
            controller,
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
            controller,
            workspace_recovery,
        })
    }
}

fn retained_submission_identity(
    source_run_id: &RunId,
    successor_run_id: &RunId,
) -> Result<(openengine_cluster_protocol::IdempotencyKey, Sha256Digest), NativeV2PlatformError> {
    let submission_key = openengine_cluster_protocol::IdempotencyKey::new(format!(
        "resume-{}",
        successor_run_id.as_str()
    ))
    .map_err(|_| NativeV2PlatformError::SubmissionIdentity)?;
    let submission_digest = Sha256Digest::new(format!(
        "{:x}",
        Sha256::digest(format!(
            "{}\0{}",
            source_run_id.as_str(),
            successor_run_id.as_str()
        ))
    ))
    .map_err(|_| NativeV2PlatformError::SubmissionIdentity)?;
    Ok((submission_key, submission_digest))
}

fn resume_secret_envelope(
    request: ResumeSecretRequest<'_>,
) -> Result<RunSecretEnvelope, NativeV2PlatformError> {
    let ResumeSecretRequest {
        admitted,
        successor_run_id,
        connections,
        connection_resolver,
        github_token,
        git_credentials,
        commit_identity,
        commit_co_author,
    } = request;
    let environment = match connection_resolver {
        Some(wire) => RunEnvironment::with_resolver_for_admitted(
            admitted,
            connections,
            crate::native_v2_hosting::build_connection_resolver(successor_run_id.clone(), wire)
                .map_err(|_| NativeV2PlatformError::ResumeCredentials)?,
        ),
        None => RunEnvironment::exact_for_admitted(admitted, connections),
    }?;
    Ok(RunSecretEnvelope {
        environment: Arc::new(environment),
        source_credentials: RunSourceCredentials {
            github_token,
            git_credentials,
            commit_identity,
            commit_co_author,
        },
    })
}

mod backend;
mod source;
use backend::{append_interrupted, append_terminal_failure};
pub(crate) use backend::submission_digest;
use source::source_credentials;

fn find_request_node(
    root: &openengine_cluster_protocol::GraphNode,
) -> Option<&openengine_cluster_protocol::NodeName> {
    if let openengine_cluster_protocol::GraphNode::Verifier(node) = root
        && node.worker.as_str() == crate::native_v2_request::ASK_WORKER_REF
    {
        return Some(&node.name);
    }
    openengine_cluster_server::graph_verifier::graph_node_children(root)
        .into_iter()
        .find_map(find_request_node)
}

impl NativeV2PlatformController {
    pub async fn inbox_show(
        &self,
        params: openengine_cluster_protocol::InboxShowParams,
    ) -> Result<openengine_cluster_protocol::InboxShowResult, NativeV2PlatformError> {
        if !self.allocator.supports_request_ask() {
            return Err(NativeV2PlatformError::InvalidRequest(
                "host does not enable openengine.request-ask/v1".to_owned(),
            ));
        }
        let now = (self
            .allocator
            .suspension_policy()
            .unwrap_or_default()
            .wall_clock)();
        let request =
            crate::v2_run_ledger::inbox::show(self.ledger.as_ref(), &params.run_id, now).await?;
        match self
            .allocator
            .workspace_recovery(&params.run_id)
            .await
            .successor_run_id
        {
            Some(successor) => Ok(crate::v2_run_ledger::inbox::with_successor(
                self.ledger.as_ref(),
                request,
                successor,
            )
            .await?),
            None => Ok(request),
        }
    }
    pub async fn inbox_list(
        &self,
        params: openengine_cluster_protocol::InboxListParams,
    ) -> Result<openengine_cluster_protocol::InboxListResult, NativeV2PlatformError> {
        if !self.allocator.supports_request_ask() {
            return Err(NativeV2PlatformError::InvalidRequest(
                "host does not enable openengine.request-ask/v1".to_owned(),
            ));
        }
        params
            .validate()
            .map_err(|error| NativeV2PlatformError::InvalidRequest(error.to_string()))?;
        let mut requests = Vec::new();
        for summary in self.list().await? {
            let stored = self
                .ledger
                .get(&summary.run_id)
                .await?
                .ok_or(RunLedgerError::RunNotFound)?;
            if stored.snapshot.phase != crate::v2_run_ledger::RunPhase::Suspended
                || !matches!(
                    stored.snapshot.suspension,
                    Some(openengine_cluster_protocol::RunSuspension::Request { .. })
                )
                || self
                    .allocator
                    .workspace_recovery(&summary.run_id)
                    .await
                    .successor_run_id
                    .is_some()
            {
                continue;
            }
            requests.push(
                self.inbox_show(openengine_cluster_protocol::InboxShowParams {
                    run_id: summary.run_id,
                })
                .await?
                .into(),
            );
        }
        Ok(crate::v2_run_ledger::inbox::page(requests, params)?)
    }
}
