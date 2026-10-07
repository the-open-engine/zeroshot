//! Run-owned preparation. Services belong to the run, never to an individual agent command.
mod output;
#[cfg(all(test, target_os = "linux"))]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::sync::Mutex as AsyncMutex;

use crate::execution::process::{HostedProcessIdentity, platform};
use crate::native_v2_cloud::{
    CapsuleAllocationUnavailable, CapsuleCleanupUnavailable, CapsulePreparation,
};
use crate::native_v2_capsule::provider_process::safe_provider_text;

const HOOK_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy)]
pub(super) enum HookPhase {
    Setup,
    Startup,
}

impl HookPhase {
    fn name(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Startup => "startup",
        }
    }
    fn failure(self) -> CapsuleAllocationUnavailable {
        match self {
            Self::Setup => CapsuleAllocationUnavailable::EnvironmentSetup,
            Self::Startup => CapsuleAllocationUnavailable::EnvironmentStartup,
        }
    }
}

/// Registered before waiting on a hook; dropping allocation retains cleanup authority here.
type OutputTask =
    Arc<AsyncMutex<Option<tokio::task::JoinHandle<Result<(), CapsuleAllocationUnavailable>>>>>;

#[derive(Default)]
pub(super) struct EnvironmentProcesses {
    processes: Mutex<Vec<Arc<HookProcess>>>,
    output: Mutex<Vec<OutputTask>>,
}

struct HookProcess {
    child: AsyncMutex<Child>,
    tree: platform::ProcessTreeHandle,
    active: AtomicBool,
}

pub(super) struct HookRequest<'a> {
    pub phase: HookPhase,
    pub script: Option<&'a str>,
    pub directory: &'a Path,
    pub identity: HostedProcessIdentity,
    pub environment: &'a BTreeMap<String, String>,
    pub preparation: &'a CapsulePreparation,
    pub redactions: &'a [String],
}

impl EnvironmentProcesses {
    pub async fn run(&self, request: HookRequest<'_>) -> Result<(), CapsuleAllocationUnavailable> {
        let Some(script) = request.script.filter(|script| !script.trim().is_empty()) else {
            return Ok(());
        };
        let phase = request.phase;
        if !cfg!(target_os = "linux") {
            return Err(phase.failure());
        }
        request
            .preparation
            .progress
            .log(&format!("Environment {} started", phase.name()))
            .await?;
        let process = self.spawn(&request, script)?;
        let outcome = process.wait(phase).await?;
        if matches!(phase, HookPhase::Setup) {
            self.finish_output().await.map_err(|_| phase.failure())?;
        }
        report_outcome(outcome, &request).await
    }

    fn spawn(
        &self,
        request: &HookRequest<'_>,
        script: &str,
    ) -> Result<Arc<HookProcess>, CapsuleAllocationUnavailable> {
        let fail = || request.phase.failure();
        let registration =
            platform::register_process_tree_for(platform::ProcessContainment::ProcessGroup)
                .map_err(|_| fail())?;
        let mut command = Command::new("/bin/bash");
        command.args([
            "--noprofile",
            "--norc",
            "-e",
            "-o",
            "pipefail",
            "-c",
            script,
        ]);
        command
            .current_dir(request.directory)
            .env_clear()
            .envs(request.environment);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        match request.phase {
            HookPhase::Setup => platform::configure_process(
                &mut command,
                platform::ProcessContainment::ProcessGroup,
            ),
            HookPhase::Startup => {
                request
                    .identity
                    .prepare_command_domain()
                    .map_err(|_| fail())?;
                request.identity.configure_command(&mut command);
            }
        }
        let mut child = command.spawn().map_err(|_| fail())?;
        // Linux capture records the child's process group without any fallible operating-system call.
        // Hooks are rejected on other platforms before spawn.
        let tree = platform::capture_process_tree(registration, &mut child)
            .expect("Linux process-tree capture is infallible");
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let process = Arc::new(HookProcess {
            child: AsyncMutex::new(child),
            tree,
            active: AtomicBool::new(true),
        });
        self.processes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(process.clone());
        let redactions = request.redactions.to_vec();
        let log = output::HookOutput::new(
            request.preparation.progress.clone(),
            redactions,
            request.phase.name(),
        );
        let mut output = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pipe) = stdout {
            output.push(Arc::new(AsyncMutex::new(Some(tokio::spawn(
                output::drain(pipe, log.clone()),
            )))));
        }
        if let Some(pipe) = stderr {
            output.push(Arc::new(AsyncMutex::new(Some(tokio::spawn(
                output::drain(pipe, log),
            )))));
        }
        Ok(process)
    }

    pub async fn cleanup(&self) -> Result<(), CapsuleCleanupUnavailable> {
        let processes = self
            .processes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for process in processes {
            if !process.active.load(Ordering::Acquire) {
                continue;
            }
            let mut child = process.child.lock().await;
            if !platform::terminate_process_tree(&process.tree, &mut child)
                .await
                .cleanup
                .proves_tree_empty()
            {
                return Err(CapsuleCleanupUnavailable);
            }
            process.active.store(false, Ordering::Release);
        }
        Ok(())
    }

    pub async fn finish_output(&self) -> Result<(), CapsuleCleanupUnavailable> {
        let tasks = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut complete = true;
        for owned in tasks {
            let mut slot = owned.lock().await;
            let Some(task) = slot.as_mut() else {
                continue;
            };
            match tokio::time::timeout_at(deadline, &mut *task).await {
                Ok(Ok(Ok(()))) => {}
                Ok(_) => complete = false,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    complete = false;
                }
            }
            // The handle stays registered across cancellation of this cleanup future.
            slot.take();
        }
        if complete {
            Ok(())
        } else {
            Err(CapsuleCleanupUnavailable)
        }
    }
}

impl HookProcess {
    async fn wait(&self, phase: HookPhase) -> Result<HookOutcome, CapsuleAllocationUnavailable> {
        let mut child = self.child.lock().await;
        let outcome = tokio::time::timeout(HOOK_TIMEOUT, child.wait()).await;
        let outcome = match outcome {
            Ok(Ok(status)) => HookOutcome::Exited(status),
            Ok(Err(_)) => HookOutcome::Unavailable,
            Err(_) => HookOutcome::Timeout,
        };
        if matches!(phase, HookPhase::Setup) || !outcome.success() {
            let cleanup = platform::terminate_process_tree(&self.tree, &mut child).await;
            if !cleanup.cleanup.proves_tree_empty() {
                return Err(phase.failure());
            }
        }
        // Startup services remain owned by the run's Environment identity after this shell is reaped.
        self.active.store(false, Ordering::Release);
        Ok(outcome)
    }
}

enum HookOutcome {
    Exited(std::process::ExitStatus),
    Unavailable,
    Timeout,
}
impl HookOutcome {
    fn success(&self) -> bool {
        matches!(self, Self::Exited(status) if status.success())
    }
}

async fn report_outcome(
    outcome: HookOutcome,
    request: &HookRequest<'_>,
) -> Result<(), CapsuleAllocationUnavailable> {
    let name = request.phase.name();
    let detail = match &outcome {
        HookOutcome::Exited(status) if status.success() => format!("Environment {name} completed"),
        HookOutcome::Exited(status) => format!("Environment {name} failed: {status}"),
        HookOutcome::Unavailable => format!("Environment {name} process became unavailable"),
        HookOutcome::Timeout => format!("Environment {name} exceeded the 15 minute timeout"),
    };
    request.preparation.progress.log(&detail).await?;
    if outcome.success() {
        Ok(())
    } else {
        Err(request.phase.failure())
    }
}

pub(super) fn tools_directory(run_root: &Path) -> PathBuf {
    run_root.join("tools")
}

pub(super) fn search_path(run_root: &Path, base: &str) -> String {
    format!("{}:{base}", tools_directory(run_root).join("bin").display())
}

pub(super) fn prepare_environment(
    run_root: &Path,
    runtime_home: &Path,
    identity: HostedProcessIdentity,
    phase: HookPhase,
) -> Result<PathBuf, CapsuleAllocationUnavailable> {
    prepare_tools(run_root, Some(identity))?;
    match phase {
        HookPhase::Setup => {
            let home = runtime_home.join("setup-home");
            crate::execution::platform::create_private_directory(&home)
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
            Ok(home)
        }
        HookPhase::Startup => identity
            .prepare_private_home(runtime_home)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime),
    }
}

pub(super) fn prepare_tools(
    run_root: &Path,
    identity: Option<HostedProcessIdentity>,
) -> Result<(), CapsuleAllocationUnavailable> {
    let tools = tools_directory(run_root);
    for directory in [&tools, &tools.join("bin")] {
        std::fs::create_dir_all(directory).map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        #[cfg(unix)]
        if let Some(identity) = identity {
            std::os::unix::fs::chown(directory, Some(identity.uid()), Some(identity.gid()))
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        }
    }
    #[cfg(not(unix))]
    let _ = identity;
    Ok(())
}
