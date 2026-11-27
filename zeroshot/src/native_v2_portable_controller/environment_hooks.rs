//! The allocator owns one hook task through both preparation and runtime cleanup.
mod process;

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use openengine_cluster_protocol::RuntimeEnvironment;
use tokio::sync::{oneshot, watch};
use tokio::time::Instant;

use crate::execution::process::wait_for_stop_request;
use crate::native_v2_capsule::provider_process::redaction_values;
use crate::native_v2_platform::{
    CapsuleAllocationRequest, CapsuleAllocationUnavailable, CapsuleCleanup,
    CapsuleCleanupUnavailable, CapsuleDestroyed, PreparationProgress,
};
use crate::native_v2_supervisor::RunRuntimeExit;

use super::{ControllerLease, PortableControllerBootstrap, WorkspaceIdentity};
use process::{HookProcess, HookShell, StartupStatus};

const PREPARATION_PHASE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub(crate) const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn has_local_teardown(definition: Option<&RuntimeEnvironment>) -> bool {
    definition
        .and_then(|definition| definition.teardown.as_deref())
        .is_some_and(|script| !script.trim().is_empty())
}

pub(super) struct EnvironmentHooks {
    shell: HookShell,
    workspace_identity: WorkspaceIdentity,
    workspace_lease: Arc<ControllerLease>,
    job: OnceLock<HookJob>,
}

struct HookJob {
    stop: watch::Sender<bool>,
    done: watch::Receiver<Option<Result<(), CapsuleCleanupUnavailable>>>,
}

impl EnvironmentHooks {
    pub(super) fn for_run(
        bootstrap: &PortableControllerBootstrap,
        workspace_identity: Option<&WorkspaceIdentity>,
        workspace_lease: Option<&Arc<ControllerLease>>,
    ) -> Option<Arc<Self>> {
        let identity = workspace_identity?;
        let definition = bootstrap.submission.environment.as_ref()?;
        if [&definition.setup, &definition.startup, &definition.teardown]
            .into_iter()
            .all(|script| {
                script
                    .as_deref()
                    .is_none_or(|script| script.trim().is_empty())
            })
        {
            return None;
        }
        let mut shell = HookShell {
            workspace: bootstrap.workspace.clone(),
            readiness: bootstrap.storage.join("startup-status"),
            environment: bootstrap
                .native_environment
                .iter()
                .filter(|(name, _)| shell_environment_name(name))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        };
        if let Some(state) = bootstrap.storage.parent().and_then(std::path::Path::parent)
            && let Ok(Some(files)) = crate::native_v2_cli::files::input_files_if_present(state)
            && let Ok(roots) = files.file_input_roots(&bootstrap.delivery_run_id)
            && !roots.is_empty()
            && let Ok(location) = files.input_location(&bootstrap.delivery_run_id)
        {
            shell.environment.insert(
                "ZEROSHOT_INPUTS".into(),
                location.to_string_lossy().into_owned(),
            );
        }
        Some(Arc::new(Self {
            shell,
            workspace_identity: identity.clone(),
            workspace_lease: workspace_lease?.clone(),
            job: OnceLock::new(),
        }))
    }

    pub(super) async fn prepare(
        self: &Arc<Self>,
        request: &CapsuleAllocationRequest<'_>,
        lost: watch::Receiver<bool>,
    ) -> Result<(), CapsuleAllocationUnavailable> {
        let definition = request
            .admitted
            .environment
            .as_ref()
            .ok_or(CapsuleAllocationUnavailable::Runtime)?;
        let values = request
            .preparation
            .environment
            .preparation_values(definition)
            .await
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let redactions = redaction_values(
            definition
                .connections
                .environment_names()
                .filter_map(|key| values.get(key.as_str()))
                .flat_map(|value| value.lines()),
        );
        let mut shell = self.shell.clone();
        shell.environment.extend(values);
        // The stored location is host-owned, independently of authored variables/connections.
        if let Some(location) = self.shell.environment.get("ZEROSHOT_INPUTS") {
            shell
                .environment
                .insert("ZEROSHOT_INPUTS".into(), location.clone());
        }
        shell
            .environment
            .insert("ZEROSHOT_RUN_ID".into(), request.run_id.as_str().into());
        let attempt = HookAttempt {
            shell,
            definition: definition.clone(),
            progress: request.preparation.progress.clone(),
            redactions,
            workspace_identity: self.workspace_identity.clone(),
            workspace_lease: self.workspace_lease.clone(),
            startup: None,
            cleanup: Ok(()),
        };
        let (ready, readiness) = oneshot::channel();
        let (stop, stopping) = watch::channel(false);
        let (finished, done) = watch::channel(None);
        // Register cleanup ownership before the task can start a subprocess.
        self.job
            .set(HookJob { stop, done })
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        tokio::spawn(async move {
            finished.send_replace(Some(attempt.run(ready, stopping, lost).await));
        });
        readiness
            .await
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?
    }

    pub(super) async fn cleanup(&self) -> Result<(), CapsuleCleanupUnavailable> {
        let Some(job) = self.job.get() else {
            return Ok(());
        };
        job.stop.send_replace(true);
        let mut done = job.done.clone();
        let outcome = done
            .wait_for(Option::is_some)
            .await
            .map_err(|_| CapsuleCleanupUnavailable)?;
        outcome.as_ref().copied().ok_or(CapsuleCleanupUnavailable)?
    }
}

fn shell_environment_name(name: &str) -> bool {
    matches!(
        name,
        "HOME"
            | "LANG"
            | "LC_ALL"
            | "LC_COLLATE"
            | "LC_CTYPE"
            | "LC_MESSAGES"
            | "LC_MONETARY"
            | "LC_NUMERIC"
            | "LC_TIME"
            | "LOGNAME"
            | "PATH"
            | "SSL_CERT_DIR"
            | "SSL_CERT_FILE"
            | "TMPDIR"
            | "USER"
    )
}

pub(super) struct HookCleanup {
    pub(super) hooks: Arc<EnvironmentHooks>,
    pub(super) inner: Arc<dyn CapsuleCleanup>,
}

#[async_trait]
impl CapsuleCleanup for HookCleanup {
    async fn destroy_or_confirm_absent(
        &self,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        self.hooks.cleanup().await?;
        self.inner.destroy_or_confirm_absent(exit).await
    }
}

struct HookAttempt {
    shell: HookShell,
    definition: RuntimeEnvironment,
    progress: Arc<dyn PreparationProgress>,
    redactions: Vec<String>,
    workspace_identity: WorkspaceIdentity,
    workspace_lease: Arc<ControllerLease>,
    startup: Option<HookProcess>,
    cleanup: Result<(), CapsuleCleanupUnavailable>,
}

impl HookAttempt {
    async fn run(
        mut self,
        ready: oneshot::Sender<Result<(), CapsuleAllocationUnavailable>>,
        stopping: watch::Receiver<bool>,
        lost: watch::Receiver<bool>,
    ) -> Result<(), CapsuleCleanupUnavailable> {
        let result = self
            .prepare(HookStop {
                stopping: Some(stopping.clone()),
                lost: Some(lost),
            })
            .await;
        let _ = ready.send(result);
        wait_for_stop_request(Some(stopping)).await;
        // Teardown gets its own budget and can still reach services from a settled startup script.
        if self.cleanup.is_ok() {
            self.teardown().await;
        } else if self.definition.teardown.is_some() {
            self.log(
                "teardown",
                "could not start because preparation cleanup was unconfirmed",
            )
            .await;
        }
        if let Some(startup) = self.startup.take() {
            startup.stop();
            self.cleanup = self.cleanup.and(startup.finish().await.map(|_| ()));
            if self.cleanup.is_err() {
                self.log("startup", "service cleanup could not be confirmed")
                    .await;
            }
        }
        self.cleanup
    }

    async fn prepare(&mut self, stopping: HookStop) -> Result<(), CapsuleAllocationUnavailable> {
        use CapsuleAllocationUnavailable::{EnvironmentSetup, EnvironmentStartup, Runtime};
        if stopping.requested() || !self.workspace_is_current() {
            return Err(Runtime);
        }
        if let Some(script) = take_script(&mut self.definition.setup) {
            self.run_finite("setup", &script, stopping.clone())
                .await
                .then_some(())
                .ok_or(EnvironmentSetup)?;
        }
        if stopping.requested() || !self.workspace_is_current() {
            return Err(Runtime);
        }
        if let Some(script) = take_script(&mut self.definition.startup) {
            self.run_startup(&script, stopping.clone())
                .await
                .then_some(())
                .ok_or(EnvironmentStartup)?;
        }
        (!stopping.requested() && self.workspace_is_current())
            .then_some(())
            .ok_or(Runtime)
    }

    fn workspace_is_current(&self) -> bool {
        self.workspace_lease.is_intact()
            && self.workspace_identity.is_current(&self.shell.workspace)
    }

    async fn teardown(&mut self) {
        if let Some(script) = take_script(&mut self.definition.teardown) {
            if self.workspace_is_current() {
                self.run_finite("teardown", &script, HookStop::default())
                    .await;
            } else {
                self.log(
                    "teardown",
                    "failed because the run workspace became unavailable",
                )
                .await;
            }
        }
    }

    async fn run_finite(&mut self, phase: &'static str, script: &str, stopping: HookStop) -> bool {
        let Some(process) = self.launch(phase, script, &stopping).await else {
            return false;
        };
        let cancellation = process.cancellation();
        let settled = process.finish();
        tokio::pin!(settled);
        let result = tokio::select! {
            biased;
            () = stopping.wait() => {
                cancellation.send_replace(true);
                settled.await
            },
            result = &mut settled => result,
        };
        self.report_settlement(phase, result).await
    }

    async fn run_startup(&mut self, script: &str, stopping: HookStop) -> bool {
        let phase = "startup";
        let Some(mut process) = self.launch(phase, script, &stopping).await else {
            return false;
        };
        let status = process
            .startup_status(&self.shell.readiness, stopping)
            .await;
        if let StartupStatus::Reported(code) = status {
            self.startup = Some(process);
            return self.report_exit(phase, code.into()).await;
        }
        process.stop();
        self.cleanup = self.cleanup.and(process.finish().await.map(|_| ()));
        self.log(phase, status.detail()).await;
        false
    }

    async fn launch(
        &mut self,
        phase: &'static str,
        script: &str,
        stopping: &HookStop,
    ) -> Option<HookProcess> {
        self.log(phase, "started").await;
        if stopping.requested() {
            self.log(phase, "was cancelled").await;
            return None;
        }
        if !self.workspace_is_current() {
            self.log(
                phase,
                "could not start because the run workspace became unavailable",
            )
            .await;
            return None;
        }
        let output = crate::native_v2_platform::hook_output::HookOutput::new(
            self.progress.clone(),
            self.redactions.clone(),
            phase,
        );
        let deadline = match phase {
            "startup" => None,
            "teardown" => Some(Instant::now() + TEARDOWN_TIMEOUT),
            _ => Some(Instant::now() + PREPARATION_PHASE_TIMEOUT),
        };
        match self.shell.launch(script, deadline, output).await {
            Ok(process) => Some(process),
            Err(error) => {
                if error.launch_evidence()
                    == crate::execution::process::ProcessLaunchEvidence::MayHaveStarted
                {
                    self.cleanup = Err(CapsuleCleanupUnavailable);
                }
                self.log(phase, "could not launch Bash").await;
                None
            }
        }
    }

    async fn report_settlement(
        &mut self,
        phase: &str,
        settled: Result<process::HookExit, CapsuleCleanupUnavailable>,
    ) -> bool {
        let Ok(exit) = settled else {
            self.cleanup = Err(CapsuleCleanupUnavailable);
            self.log(phase, "cleanup could not be confirmed").await;
            return false;
        };
        match exit {
            process::HookExit::Code(code) => self.report_exit(phase, code).await,
            process::HookExit::Interrupted(reason) => {
                self.log(phase, reason).await;
                false
            }
        }
    }

    async fn report_exit(&self, phase: &str, code: i32) -> bool {
        if code == 0 {
            self.log(phase, "completed").await;
        } else {
            self.log(phase, &format!("failed with exit code {code}"))
                .await;
        }
        code == 0
    }

    async fn log(&self, phase: &str, detail: &str) {
        let _ = self
            .progress
            .log(&format!("Environment {phase} {detail}"))
            .await;
    }
}

fn take_script(script: &mut Option<String>) -> Option<String> {
    script.take().filter(|script| !script.trim().is_empty())
}

#[derive(Clone, Default)]
struct HookStop {
    stopping: Option<watch::Receiver<bool>>,
    lost: Option<watch::Receiver<bool>>,
}

impl HookStop {
    fn requested(&self) -> bool {
        self.stopping
            .iter()
            .chain(self.lost.iter())
            .any(|signal| *signal.borrow())
    }

    async fn wait(self) {
        tokio::select! {
            () = wait_for_stop_request(self.stopping) => {},
            () = wait_for_stop_request(self.lost) => {},
        }
    }
}
