use std::sync::Arc;
use std::io::Write;

use async_trait::async_trait;
use openengine_cluster_protocol::RunId;
use tokio::sync::watch;

use crate::native_v2_admission::DeliveryPolicy;
use crate::native_v2_cloud::{
    CapsuleCleanup, CapsuleCleanupUnavailable, CapsuleDestroyed, ExclusiveControllerClaim,
};
use crate::native_v2_runner::NodeRunner;
use crate::native_v2_observability::NativeV2Observability;
use crate::native_v2_target_authority::{NewOperatorDiagnostic, OperatorDiagnosticStore};
use crate::native_v2_supervisor::{RunEnvironment, RunRuntimeExit};
use crate::v2_run_ledger::RunLedger;

pub struct PortableRuntime {
    pub runner: Arc<dyn NodeRunner>,
    pub cleanup: Arc<dyn CapsuleCleanup>,
}

/// The shared one-run execution boundary used by hosted and standalone controllers.
pub struct PortableRunEngine {
    supervisor: Arc<crate::native_v2_supervisor::NativeV2Supervisor>,
    removable: watch::Receiver<bool>,
}

pub struct PortableRunEngineBootstrap {
    pub run_id: RunId,
    pub ledger: Arc<dyn RunLedger>,
    pub environment: RunEnvironment,
    pub runtime: PortableRuntime,
    pub loss: watch::Receiver<bool>,
    pub controller_claim: Arc<dyn ExclusiveControllerClaim>,
    pub delivery_policy: DeliveryPolicy,
    pub observability: NativeV2Observability,
    pub(crate) operator_diagnostics: Arc<OperatorDiagnosticStore>,
}

impl PortableRunEngine {
    #[must_use]
    pub fn start(bootstrap: PortableRunEngineBootstrap) -> Arc<Self> {
        let PortableRunEngineBootstrap {
            run_id,
            ledger,
            environment,
            runtime,
            mut loss,
            controller_claim,
            delivery_policy,
            observability,
            operator_diagnostics,
        } = bootstrap;
        let supervisor = Arc::new(
            crate::native_v2_supervisor::NativeV2Supervisor::new(
                run_id.clone(),
                ledger,
                runtime.runner,
                Arc::new(environment),
            )
            .with_delivery_policy(delivery_policy)
            .with_live_output(Arc::new(observability.clone()))
            .with_runtime_cleanup(Arc::new(PortableRuntimeCleanup(runtime.cleanup))),
        );
        let (removable_sender, removable) = watch::channel(false);
        let engine = Arc::new(Self {
            supervisor: supervisor.clone(),
            removable,
        });
        tokio::spawn(async move {
            let _controller_claim = controller_claim;
            let drive_supervisor = supervisor.clone();
            let result = tokio::spawn(async move {
                let drive = drive_supervisor.drive();
                tokio::pin!(drive);
                tokio::select! {
                    result = &mut drive => result,
                    () = wait_for_runtime_loss(&mut loss) => {
                        drive_supervisor.runtime_lost().await;
                        drive.await
                    }
                }
            })
            .await;
            match task_result(result) {
                Ok(_) => observability.runtime_finished(&run_id),
                Err(cause) => {
                    let recovery = task_result(
                        tokio::spawn(async move { supervisor.fail_runtime().await }).await,
                    );
                    let mut details = format!("supervisor.drive: {cause}");
                    if let Err(error) = recovery {
                        details.push_str(&format!("\nFailure cleanup/persistence: {error}"));
                    }
                    // These are typed platform errors. Never render a panic payload, provider
                    // response, environment value, or arbitrary SQLite query/trigger text here.
                    let _ = writeln!(
                        std::io::stderr(),
                        "run {} runtime_failed: {details}",
                        run_id.as_str()
                    );
                    operator_diagnostics.record(NewOperatorDiagnostic {
                        run_id: run_id.clone(),
                        code: "runtime_failed",
                        operation: "supervisor.drive",
                        exit_status: None,
                        stdout: String::new(),
                        stderr: details,
                        stdout_truncated: false,
                        stderr_truncated: false,
                    });
                    observability.runtime_failed(&run_id).await;
                }
            }
            // Removal follows an explicit observed outcome, never a dropped false sender.
            removable_sender.send_replace(true);
        });
        engine
    }

    pub async fn force_stop(
        &self,
    ) -> Result<(), crate::native_v2_supervisor::NativeV2SupervisorError> {
        self.supervisor.force_stop().await?;
        self.supervisor.drive().await.map(|_| ())
    }

    pub async fn wait_removable(&self) -> bool {
        wait_for_removability(self.removable.clone()).await
    }
}

async fn wait_for_removability(mut removable: watch::Receiver<bool>) -> bool {
    while !*removable.borrow_and_update() && removable.changed().await.is_ok() {}
    *removable.borrow()
}

fn task_result<T>(
    result: Result<
        Result<T, crate::native_v2_supervisor::NativeV2SupervisorError>,
        tokio::task::JoinError,
    >,
) -> Result<T, String> {
    match result {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(error) if error.is_panic() => Err("supervisor task panicked".to_owned()),
        Err(_) => Err("supervisor task was cancelled".to_owned()),
    }
}

async fn wait_for_runtime_loss(loss: &mut watch::Receiver<bool>) {
    loop {
        if *loss.borrow_and_update() || loss.changed().await.is_err() {
            return;
        }
    }
}

struct PortableRuntimeCleanup(Arc<dyn CapsuleCleanup>);

#[async_trait]
impl crate::native_v2_supervisor::RunRuntimeCleanup for PortableRuntimeCleanup {
    async fn cleanup(
        &self,
        exit: RunRuntimeExit,
    ) -> Result<(), crate::native_v2_supervisor::RuntimeCleanupUnavailable> {
        self.0
            .destroy_or_confirm_absent(exit)
            .await
            .map(|_| ())
            .map_err(|_| crate::native_v2_supervisor::RuntimeCleanupUnavailable)
    }
}

impl PortableRuntime {
    #[must_use]
    pub fn new(runner: Arc<dyn NodeRunner>) -> Self {
        Self {
            runner,
            cleanup: Arc::new(ConfirmedCleanup),
        }
    }

    #[must_use]
    pub fn with_cleanup(runner: Arc<dyn NodeRunner>, cleanup: Arc<dyn CapsuleCleanup>) -> Self {
        Self { runner, cleanup }
    }
}

struct ConfirmedCleanup;

#[async_trait]
impl CapsuleCleanup for ConfirmedCleanup {
    async fn destroy_or_confirm_absent(
        &self,
        _exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        Ok(CapsuleDestroyed::confirmed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn removal_requires_explicit_completion_even_when_the_sender_disappears() {
        for completed in [false, true] {
            let (sender, receiver) = watch::channel(false);
            if completed {
                sender.send_replace(true);
            }
            drop(sender);
            assert_eq!(wait_for_removability(receiver).await, completed);
        }
    }

    #[tokio::test]
    async fn cancelled_supervisor_task_is_an_explicit_failure() {
        let task = tokio::spawn(std::future::pending::<
            Result<(), crate::native_v2_supervisor::NativeV2SupervisorError>,
        >());
        task.abort();
        assert_eq!(
            task_result(task.await),
            Err("supervisor task was cancelled".to_owned())
        );
    }
}
