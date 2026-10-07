//! A preparation task owns acceptance, cancellation and cleanup until graph handoff.
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures_util::FutureExt;

use super::*;
use crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection;
use crate::v2_run_ledger::{SafeLogLine, SafeLogStream};

const PREPARATION_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub(super) struct PreparationStart {
    pub stored: StoredRun,
    pub secrets: RunSecretEnvelope,
    pub controller_claim: Arc<dyn ExclusiveControllerClaim>,
    pub resume: Option<ResumePreparation>,
}

pub(super) struct ResumePreparation {
    pub source_run_id: RunId,
    pub selection: CheckpointRestoreSelection,
}

pub(super) struct PreparationTask {
    cancellation: watch::Sender<bool>,
    finished: watch::Sender<bool>,
    failure: Mutex<Option<PreparationFailure>>,
    cleanup_turn: Mutex<()>,
    // Retained after failed cleanup; replay or force can retry without releasing the run lease.
    controller_claim: Arc<dyn ExclusiveControllerClaim>,
}

#[derive(Clone, Copy)]
struct PreparationFailure {
    reason: &'static str,
    exit: RunRuntimeExit,
}

impl PreparationFailure {
    fn unavailable(reason: &'static str) -> Self {
        Self {
            reason,
            exit: RunRuntimeExit::RuntimeLost,
        }
    }

    fn cancelled() -> Self {
        Self {
            reason: "force_stopped",
            exit: RunRuntimeExit::ForceStopped,
        }
    }
}

impl PreparationTask {
    pub(super) fn is_finished(&self) -> bool {
        *self.finished.borrow()
    }

    async fn wait(&self) {
        let mut finished = self.finished.subscribe();
        let _ = finished.wait_for(|value| *value).await;
    }
}

struct LedgerPreparationProgress {
    ledger: Arc<dyn RunLedger>,
    run_id: RunId,
}

#[async_trait]
impl PreparationProgress for LedgerPreparationProgress {
    async fn log(&self, line: &str) -> Result<(), CapsuleAllocationUnavailable> {
        let timestamp = crate::native_v2_runner::current_timestamp();
        let line = SafeLogLine::new(line).map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        self.ledger
            .append(
                &self.run_id,
                vec![RunEvent::SafeLog {
                    execution: None,
                    timestamp,
                    stream: SafeLogStream::System,
                    line,
                }],
            )
            .await
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        Ok(())
    }
}

impl NativeV2CloudController {
    pub(super) async fn start_preparation(
        &self,
        start: PreparationStart,
    ) -> Result<(), NativeV2CloudError> {
        self.observability.track_runtime(&start.stored.snapshot)?;
        let run_id = start.stored.snapshot.run_id.clone();
        let task = Arc::new(PreparationTask {
            cancellation: watch::channel(false).0,
            finished: watch::channel(false).0,
            failure: Mutex::new(None),
            cleanup_turn: Mutex::new(()),
            controller_claim: start.controller_claim.clone(),
        });
        self.runtimes
            .lock()
            .await
            .insert(run_id.clone(), RuntimeSlot::Preparing(task.clone()));
        let controller = self.clone();
        tokio::spawn(async move {
            let result = AssertUnwindSafe(controller.prepare_run(start, &task))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err(PreparationFailure::unavailable("runtime_unavailable")));
            if let Err(failure) = result {
                *task.failure.lock().await = Some(failure);
                let cleanup =
                    AssertUnwindSafe(controller.retry_preparation_cleanup(&run_id, &task, false))
                        .catch_unwind()
                        .await;
                if !matches!(cleanup, Ok(Ok(()))) {
                    controller.observability.runtime_failed(&run_id);
                }
            }
            task.finished.send_replace(true);
        });
        Ok(())
    }

    async fn prepare_run(
        &self,
        start: PreparationStart,
        task: &PreparationTask,
    ) -> Result<(), PreparationFailure> {
        let mut cancellation = task.cancellation.subscribe();
        let progress: Arc<dyn PreparationProgress> = Arc::new(LedgerPreparationProgress {
            ledger: self.ledger.clone(),
            run_id: start.stored.snapshot.run_id.clone(),
        });
        let allocation = AssertUnwindSafe(self.prepare_capsule(&start, progress)).catch_unwind();
        let capsule = tokio::select! {
            biased;
            _ = cancellation.wait_for(|value| *value) => return Err(PreparationFailure::cancelled()),
            _ = tokio::time::sleep(PREPARATION_TIMEOUT) => {
                return Err(PreparationFailure::unavailable("environment_preparation_timeout"));
            }
            result = allocation => match result {
                Ok(result) => result?,
                Err(_) => return Err(PreparationFailure::unavailable("runtime_unavailable")),
            },
        };
        // Handoff is not interruptible: force waits for it, then signals the owning engine.
        if *task.cancellation.borrow() {
            return Err(PreparationFailure::cancelled());
        }
        self.start_allocated(AllocatedRunStart {
            stored: start.stored,
            environment: start.secrets.environment,
            controller_claim: task.controller_claim.clone(),
            capsule,
        })
        .await
        .map_err(|_| PreparationFailure::unavailable("runtime_unavailable"))?;
        Ok(())
    }

    async fn prepare_capsule(
        &self,
        start: &PreparationStart,
        progress: Arc<dyn PreparationProgress>,
    ) -> Result<AllocatedCapsule, PreparationFailure> {
        progress
            .log("Preparing execution environment")
            .await
            .map_err(|error| PreparationFailure::unavailable(error.failure_code()))?;
        let github_token = source_github_token(&start.secrets)
            .await
            .map_err(|_| PreparationFailure::unavailable("runtime_unavailable"))?;
        let preparation = CapsulePreparation {
            environment: start.secrets.environment.as_ref().clone(),
            progress,
        };
        let run_id = &start.stored.snapshot.run_id;
        if let Some(resume) = &start.resume {
            self.allocator
                .allocate_from_retained(RetainedAllocationRequest {
                    run_id,
                    admitted: &start.stored.admitted,
                    github_token: github_token.as_deref(),
                    preparation,
                    selection: resume.selection.clone(),
                    source_run_id: &resume.source_run_id,
                })
                .await
                .map_err(|error| match error {
                    RetainedAllocationUnavailable::Settled(error) => {
                        PreparationFailure::unavailable(error.failure_code())
                    }
                    RetainedAllocationUnavailable::CleanupUnconfirmed(_) => {
                        PreparationFailure::unavailable("runtime_unavailable")
                    }
                })
        } else {
            self.allocator
                .allocate(CapsuleAllocationRequest {
                    run_id,
                    admitted: &start.stored.admitted,
                    github_token: github_token.as_deref(),
                    preparation,
                })
                .await
                .map_err(|error| PreparationFailure::unavailable(error.failure_code()))
        }
    }

    pub(super) async fn reconcile_owned_preparation(
        &self,
        run_id: &RunId,
        runtime: RuntimeSlot,
    ) -> Result<(), NativeV2CloudError> {
        if let RuntimeSlot::Preparing(task) = runtime {
            if task.is_finished() {
                return self.retry_preparation_cleanup(run_id, &task, false).await;
            }
        }
        Ok(())
    }

    pub(super) async fn retry_preparation_cleanup(
        &self,
        run_id: &RunId,
        task: &PreparationTask,
        force: bool,
    ) -> Result<(), NativeV2CloudError> {
        let _turn = task.cleanup_turn.lock().await;
        if self
            .ledger
            .get(run_id)
            .await?
            .is_some_and(|stored| stored.snapshot.terminal.is_some())
        {
            self.observability.runtime_finished(run_id);
            self.runtimes.lock().await.remove(run_id);
            return Ok(());
        }
        let failure = if force {
            PreparationFailure::cancelled()
        } else {
            task.failure
                .lock()
                .await
                .unwrap_or(PreparationFailure::unavailable("runtime_lost"))
        };
        self.allocator
            .destroy_or_confirm_absent(run_id, failure.exit)
            .await
            .map_err(|_| NativeV2SupervisorError::RuntimeCleanup(RuntimeCleanupUnavailable))?;
        if force || failure.reason == "force_stopped" {
            self.ledger.request_force_stop(run_id).await?;
        }
        self.append_unavailable(run_id, failure.reason).await?;
        self.runtimes.lock().await.remove(run_id);
        Ok(())
    }

    pub(super) async fn force_preparing(
        &self,
        run_id: &RunId,
        task: &PreparationTask,
    ) -> Result<(), NativeV2CloudError> {
        task.cancellation.send_replace(true);
        task.wait().await;
        let runtime = self.runtimes.lock().await.get(run_id).cloned();
        match runtime {
            Some(RuntimeSlot::Running(engine)) => self.force_running(run_id, &engine).await,
            Some(RuntimeSlot::Preparing(_)) => {
                self.retry_preparation_cleanup(run_id, task, true).await
            }
            None => Ok(()),
        }
    }
}
