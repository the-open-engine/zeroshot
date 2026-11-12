//! Auth-free local CLI backend for one-run controller processes.

use std::path::{Path, PathBuf};
use std::fs::{File, OpenOptions};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use fs2::FileExt;
use openengine_cluster_client::{ClientError, ClusterClient};
use openengine_cluster_protocol::{
    ConnectionDeleteRequest, ConnectionDeleteResult, ConnectionListRequest, ConnectionListResult,
    ConnectionMutationResult, ConnectionSetRequest, IdempotencyKey, RunAttachEventNotification,
    RunAttachParams, RunForceParams, RunForceResult, RunId, RunListParams, RunListResult,
    RunLogEventNotification, RunLogsParams, RunProfile, RunProfileDefaultRequest,
    RunProfileDefaultResult, RunProfileDeleteResult, RunProfileListRequest, RunProfileListResult,
    RunProfileMutationResult, RunProfileSelector, RunProfileSetRequest, RunStatusParams,
    RunStatusResult, RunSubmitResult, RunWatchParams, Sha256Digest, RunSubmission, TerminalResult,
    WorkspaceRecovery,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::time::{Instant, sleep};

use super::oecp::{ChannelSubscription, spawn_attach, spawn_logs, spawn_watch};
use super::support::{CommitPaths, cleanup_temporary, nonempty_environment, write_and_commit};
use super::{
    CliRunForceResult, CliRunListResult, CliRunStatusResult, CliRunWatchEventNotification,
    CliProfileListResult, LocalRunProfileStore, NativeV2CliBackend, NativeV2CliError,
    PreparedRunRequest, TargetAdd, default_local_state_root,
};
use crate::execution::platform::{ControllerChild as Child, FileAccess, private_file};
use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::native_v2_platform::submission_digest;
use crate::native_v2_local::{PreparedLocalRun, prepare_local_run};
use crate::native_v2_supervisor::RunEnvironment;
use crate::native_v2_portable_controller::{
    ControllerLease, ControllerLeaseError, DEFAULT_PREPARATION_TIMEOUT,
    PortableControllerBootstrap, PortableControllerPaths, PortableRunController, read_ready,
    take_startup_failure, write_bootstrap_file,
};
use crate::native_v2_portable_controller::process::{PortableControllerTransport, connect_transport};
use crate::v2_run_ledger::sqlite::SqliteRunLedger;
use crate::v2_run_ledger::RunLedger;

#[path = "local/state.rs"]
mod state;
use state::*;

#[path = "local/connections.rs"]
mod connections;
use connections::LocalConnectionStore;

#[path = "local/backend.rs"]
mod backend;

#[path = "local/inspection.rs"]
mod inspection;

#[path = "local/branch.rs"]
mod branch;
#[path = "local/delivery_recovery.rs"]
mod delivery_recovery;
pub(crate) use delivery_recovery::review_binding_recorder;

#[path = "local/native_forge.rs"]
pub(super) mod native_forge;
#[path = "local/submission.rs"]
mod submission;

/// Private process mode intercepted by the shipped binary before public CLI parsing.
#[doc(hidden)]
pub const LOCAL_CONTROLLER_MODE: &str = "__zeroshot-run-controller";

const BOOTSTRAP_FILE: &str = "controller.bootstrap.json";
const SUBMISSION_LOCK_FILE: &str = "submission.lock";
/// Bounds controller handoffs: reconnecting to a live controller and reconciling a successor
/// whose launcher may still be starting it. Preparation before readiness has its own deadline.
const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(10);
const PREPARATION_TIMEOUT_ENV: &str = "ZEROSHOT_PREPARATION_TIMEOUT";
const MAX_PREPARATION_TIMEOUT_SECONDS: u64 = 86_400;
/// Lets a controller that reached its own preparation deadline stop its helpers, record why, and
/// exit before the waiting CLI stops it.
const CONTROLLER_STOP_GRACE: Duration = Duration::from_secs(30);
const STALE_BOOTSTRAP_AGE: Duration = Duration::from_secs(60);
const CONTROLLER_HANDOFF_RETRY_DELAY: Duration = Duration::from_millis(25);
const RECOVERY_FILE: &str = "workspace-recovery.json";
const RECOVERY_CLAIM_FILE: &str = "workspace-recovery.claim";
const MAX_RECOVERY_LINEAGE_DEPTH: usize = 64;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LocalRecoveryDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery_receipt: Option<serde_json::Value>,
    submission: RunSubmission,
    workspace: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invoking_workspace: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_digest: Option<Sha256Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_revision: Option<openengine_cluster_protocol::SourceRevisionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resumed_from: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    successor_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_restore: Option<PendingRestore>,
}

/// A checkpoint restore recorded before its successor launched. It stays until a successor
/// becomes ready, because a controller stopped during replacement can leave a partial checkout
/// that only a complete restore repairs. `attempt` owns the private stage beside the workspace.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PendingRestore {
    selection: crate::native_v2_supervisor::checkpoints::CheckpointRestoreSelection,
    snapshot: crate::native_v2_supervisor::checkpoints::filesystem::SnapshotId,
    attempt: RunId,
}

/// A failed controller start. `controller_stopped` is false only when a launched controller
/// could not be confirmed to have exited, so its claim must be reconciled like an unknown one.
struct StartFailure {
    error: NativeV2CliError,
    controller_stopped: bool,
}

impl From<NativeV2CliError> for StartFailure {
    fn from(error: NativeV2CliError) -> Self {
        Self {
            error,
            controller_stopped: true,
        }
    }
}

enum ControllerWait {
    Ready,
    Exited(Option<String>),
    TimedOut,
}

struct LocalRecoveryClaim(File);

impl Drop for LocalRecoveryClaim {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

#[derive(Clone, Debug)]
pub struct LocalCliBackend {
    state_root: PathBuf,
    executable: PathBuf,
    current_directory: PathBuf,
    git_program: PathBuf,
    ready_timeout: Duration,
    preparation_timeout: Duration,
}

impl LocalCliBackend {
    pub fn production() -> Result<Self, NativeV2CliError> {
        let backend = Self {
            state_root: default_local_state_root()?,
            executable: std::env::current_exe().map_err(local_io)?,
            current_directory: std::env::current_dir().map_err(local_io)?,
            git_program: PathBuf::from("git"),
            ready_timeout: DEFAULT_READY_TIMEOUT,
            preparation_timeout: preparation_timeout(nonempty_environment(
                PREPARATION_TIMEOUT_ENV,
            ))?,
        };
        Ok(backend)
    }

    #[must_use]
    pub fn new(
        state_root: PathBuf,
        executable: PathBuf,
        current_directory: PathBuf,
        git_program: PathBuf,
    ) -> Self {
        Self {
            state_root,
            executable,
            current_directory,
            git_program,
            ready_timeout: DEFAULT_READY_TIMEOUT,
            preparation_timeout: DEFAULT_PREPARATION_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_ready_timeout(mut self, timeout: Duration) -> Self {
        self.ready_timeout = timeout;
        self
    }

    /// Bounds controller preparation before readiness, including any checkpoint restore.
    #[must_use]
    pub fn with_preparation_timeout(mut self, timeout: Duration) -> Self {
        self.preparation_timeout = timeout;
        self
    }

    fn run_storage(
        &self,
        run_id: &openengine_cluster_protocol::RunId,
    ) -> Result<PathBuf, NativeV2CliError> {
        validate_local_run_id(run_id)?;
        Ok(self.state_root.join("runs").join(run_id.as_str()))
    }

    fn paths(
        &self,
        run_id: &openengine_cluster_protocol::RunId,
    ) -> Result<PortableControllerPaths, NativeV2CliError> {
        let paths = self.run_storage(run_id).map(PortableControllerPaths::new)?;
        validate_local_socket_path(&paths.socket())?;
        Ok(paths)
    }

    // Control operations alone may recover a controller after confirmed loss.
    async fn connect_run(
        &self,
        run_id: &RunId,
    ) -> Result<Arc<PortableControllerTransport>, NativeV2CliError> {
        use crate::native_v2_portable_controller::availability::{
            ControllerAvailability, check_controller, connect_controller,
        };
        let paths = self.existing_run_paths(run_id)?;
        if read_ready(&paths).is_ok_and(|ready| ready.run_id != *run_id) {
            return Err(local_message(
                "controller readiness has a different run identity",
            ));
        }
        // A live controller stops owned work before it waits for storage, so a control request
        // needs an endpoint that accepts it, never a status reply first. An endpoint that
        // accepts nothing is taken over only after its loss is confirmed below.
        if let Some(transport) = connect_controller(&paths, run_id).await {
            return Ok(transport);
        }
        match check_controller(&paths, run_id).await {
            ControllerAvailability::Responding(transport, _) => return Ok(transport),
            ControllerAvailability::Unavailable(observed)
                if observed.state == openengine_cluster_protocol::ControllerState::Lost => {}
            _ => {
                return Err(local_message(format!(
                    "controller availability is unknown; run {} is still owned or unreachable",
                    run_id.as_str()
                )));
            }
        }
        let server = Arc::new(
            PortableRunController::confirm_loss(paths.clone(), run_id.clone())
                .await
                .map_err(local_error)?,
        )
        .bind()
        .await
        .map_err(local_error)?;
        tokio::spawn(async move {
            let _ = server.serve().await;
        });
        connect_transport(&paths).await.map_err(local_error)
    }

    fn existing_run_paths(
        &self,
        run_id: &openengine_cluster_protocol::RunId,
    ) -> Result<PortableControllerPaths, NativeV2CliError> {
        if !self.run_storage(run_id)?.is_dir() {
            return Err(NativeV2CliError::RunNotFound {
                run_id: run_id.as_str().to_owned(),
            });
        }
        self.paths(run_id)
    }

    async fn start_controller(
        &self,
        request: PreparedRunRequest,
    ) -> Result<openengine_cluster_protocol::RunId, NativeV2CliError> {
        NativeV2Admission
            .validate_intent(&request.intent, DeliveryPolicy::Optional)
            .await
            .map_err(NativeV2CliError::InvalidRun)?;

        let worktree = request.worktree;
        let prepared = self.prepare_run(request).await?;
        self.submit_prepared(prepared, worktree).await
    }

    async fn prepare_run(
        &self,
        mut request: PreparedRunRequest,
    ) -> Result<PreparedLocalRun, NativeV2CliError> {
        let native_environment = native_forge::capture_native_environment(&self.current_directory)?;
        self.enrich_local_source(&mut request, &native_environment)
            .await?;
        let (workspace, source) = crate::native_v2_local::prepare_local_source(
            &request,
            &self.current_directory,
            &self.git_program,
        )
        .map_err(|error| {
            self.local_branch_error(
                error,
                request.cwd.as_deref().unwrap_or(&self.current_directory),
            )
        })?;
        crate::native_v2_local::validate_workspace_state(&workspace, &self.state_root)
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
        let mut submission =
            crate::native_v2_local::local_submission(&request.intent, source.clone());
        native_forge::materialize_local_access(
            &mut submission,
            native_forge::NativeContext {
                environment: &native_environment,
                directory: &workspace,
            },
            request.github_token.is_some(),
        )?;
        request.intent.runtime = submission.runtime.clone();
        let store = LocalConnectionStore::new(self.state_root.clone());
        native_forge::fill_missing(
            &submission,
            &mut request.connections,
            &store,
            native_forge::NativeContext {
                environment: &native_environment,
                directory: &workspace,
            },
        )
        .await?;
        request.connections = store
            .resolve_submission(&submission, &request.connections)?
            .bootstrap_values();
        request.git_credentials =
            RunEnvironment::exact_for_submission(&submission, request.connections.clone())?
                .resolve_git_source()
                .await?;

        request.cwd = Some(workspace);
        request.source = source.map(|resolved| super::NamedRunSource {
            resolved,
            dirty: false,
        });
        prepare_local_run(request, &self.current_directory, &self.git_program).map_err(local_error)
    }

    async fn enrich_local_source(
        &self,
        request: &mut PreparedRunRequest,
        environment: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), NativeV2CliError> {
        let selected = request.cwd.as_deref().unwrap_or(&self.current_directory);
        let selected = if selected.is_absolute() {
            selected.to_owned()
        } else {
            self.current_directory.join(selected)
        };
        if request.source.is_none() {
            let (_, source) =
                crate::native_v2_local::local_resolved_source(&selected, &self.git_program)
                    .map_err(local_error)?;
            request.source = source.map(|mut resolved| {
                if let Some(remote) = &mut resolved.remote {
                    remote.connection = request.source_connection.clone();
                }
                super::NamedRunSource {
                    resolved,
                    dirty: false,
                }
            });
        }
        let capabilities =
            crate::native_v2_admission::delivery_capabilities(&request.intent.graph.root);
        let needs_forge = native_forge::needs_forge(&capabilities);
        if needs_forge && let Some(source) = &mut request.source {
            native_forge::enrich_source(
                &mut source.resolved,
                native_forge::NativeContext {
                    environment,
                    directory: &selected,
                },
            )
            .await?;
        }
        Ok(())
    }

    async fn start_prepared_controller(
        &self,
        prepared: PreparedLocalRun,
    ) -> Result<RunId, NativeV2CliError> {
        self.start_prepared_controller_with_lineage(prepared, None, None)
            .await
            .map_err(|failure| failure.error)
    }

    async fn start_prepared_controller_with_lineage(
        &self,
        prepared: PreparedLocalRun,
        resumed_from: Option<RunId>,
        checkpoint: Option<crate::native_v2_supervisor::checkpoints::CheckpointRestore>,
    ) -> Result<RunId, StartFailure> {
        // Checkpoint restoration replaces this workspace under the controller's lease. A relative
        // executable may exist only in that snapshot, so the controller checks lanes and delivery
        // when it composes the restored run, before admission, and its startup report names them.
        if checkpoint.is_none() {
            crate::native_v2_local::check_lane_executables(
                &prepared.submission.runtime,
                &prepared.native_environment,
                &prepared.workspace,
            )
            .and_then(|()| {
                crate::native_v2_local::check_delivery_executables(
                    &prepared.submission,
                    &prepared.native_environment,
                    &prepared.workspace,
                )
            })
            .map_err(|error| NativeV2CliError::Usage(error.to_string()))?;
        }
        let delivery_receipt = match resumed_from.as_ref() {
            Some(predecessor) => {
                self.recover_delivery_receipt(predecessor, &prepared)
                    .await?
            }
            None => None,
        };
        let adopt_existing_delivery = resumed_from.is_some();
        let paths = self.paths(&prepared.run_id)?;
        let ready_timeout = self
            .preparation_timeout
            .saturating_add(CONTROLLER_STOP_GRACE)
            .saturating_add(
                crate::native_v2_admission::comment_node(&prepared.submission.graph.root)
                    .map_or(Duration::ZERO, |_| {
                        crate::native_v2_review::LOCAL_CHECKOUT_TIMEOUT
                    }),
            );
        let storage = self.create_run_storage(&prepared.run_id)?;
        self.write_recovery_document(
            &prepared.run_id,
            &LocalRecoveryDocument {
                delivery_receipt: delivery_receipt.clone(),
                local_branch: prepared.local_branch.clone(),
                invoking_workspace: prepared.invoking_workspace.clone(),
                request_digest: prepared.request_digest.clone(),
                workspace_revision: prepared.workspace_revision.clone(),
                submission: prepared.submission.clone(),
                workspace: prepared.workspace.clone(),
                delivery_run_id: Some(prepared.delivery_run_id.clone()),
                resumed_from,
                successor_run_id: None,
                pending_restore: None,
            },
        )?;
        let workspace_lease = self.workspace_lease(&prepared.workspace)?;
        let checkpoint_repository = self.checkpoint_repository(&prepared.delivery_run_id)?;
        let bootstrap_path = storage.join(BOOTSTRAP_FILE);
        let bootstrap = PortableControllerBootstrap {
            delivery_receipt,
            local_branch: prepared.local_branch.clone(),
            memory_storage: Some(
                self.state_root
                    .join("active-memory")
                    .join("lessons.sqlite3"),
            ),
            checkpoint,
            preparation_timeout: self.preparation_timeout,
            run_id: prepared.run_id.clone(),
            delivery_run_id: prepared.delivery_run_id,
            adopt_existing_delivery,
            submission: prepared.submission,
            environment: prepared.environment,
            native_environment: prepared.native_environment,
            github_token: prepared.github_token,
            github_token_file: prepared.github_token_file,
            delivery_validator: prepared.delivery_validator,
            git_credentials: prepared.git_credentials,
            workspace: prepared.workspace,
            workspace_lease,
            checkpoint_repository,
            storage,
            delivery_policy: DeliveryPolicy::Optional,
        };
        write_bootstrap_file(&bootstrap_path, &bootstrap).map_err(local_error)?;
        let mut child = match self.spawn_controller(&bootstrap_path) {
            Ok(child) => child,
            Err(error) => {
                remove_private_bootstrap(&bootstrap_path);
                return Err(error.into());
            }
        };
        let started = Instant::now();
        let wait = wait_for_controller(&mut child, &paths, &prepared.run_id, ready_timeout).await;
        if matches!(wait, Ok(ControllerWait::Ready)) {
            return Ok(prepared.run_id);
        }
        let controller_stopped = matches!(wait, Ok(ControllerWait::Exited(_)))
            || crate::execution::platform::stop_controller(&mut child)
                .await
                .is_ok();
        remove_private_bootstrap(&bootstrap_path);
        // A stopped controller may still have recorded why it had not become ready.
        let recorded = take_startup_failure(&paths);
        let error = match wait {
            Ok(ControllerWait::Exited(reason)) => {
                self.controller_exit_error(reason.or(recorded), started.elapsed())
            }
            Ok(ControllerWait::TimedOut) => {
                self.preparation_timeout_error(recorded, started.elapsed(), controller_stopped)
            }
            Ok(ControllerWait::Ready) => unreachable!("a ready controller returned above"),
            Err(error) => error,
        };
        Err(StartFailure {
            error,
            controller_stopped,
        })
    }

    fn controller_exit_error(&self, reason: Option<String>, elapsed: Duration) -> NativeV2CliError {
        let mut message = match reason {
            Some(reason) => format!("controller exited before becoming ready: {reason}"),
            None => "controller exited before becoming ready".to_owned(),
        };
        // The controller measures the same deadline from its own, later start.
        if elapsed >= self.preparation_timeout {
            message.push_str(&format!(
                "; {PREPARATION_TIMEOUT_ENV} sets its {} s preparation deadline",
                self.preparation_timeout.as_secs()
            ));
        }
        local_message(message)
    }

    fn preparation_timeout_error(
        &self,
        reason: Option<String>,
        elapsed: Duration,
        stopped: bool,
    ) -> NativeV2CliError {
        let outcome = if stopped {
            "was stopped"
        } else {
            "could not be confirmed stopped"
        };
        let mut message = format!(
            "controller did not become ready within its {} s preparation deadline ({PREPARATION_TIMEOUT_ENV}) and {outcome} after {:.1} s",
            self.preparation_timeout.as_secs(),
            elapsed.as_secs_f64()
        );
        if let Some(reason) = reason {
            message.push_str(": ");
            message.push_str(&reason);
        }
        local_message(message)
    }

    async fn acquire_submission_lock(&self) -> Result<ControllerLease, NativeV2CliError> {
        self.remove_stale_bootstraps(STALE_BOOTSTRAP_AGE)?;
        let path = self.state_root.join(SUBMISSION_LOCK_FILE);
        loop {
            match ControllerLease::acquire(&path) {
                Ok(lock) => return Ok(lock),
                Err(ControllerLeaseError::Held) => sleep(Duration::from_millis(20)).await,
                Err(error) => return Err(local_error(error)),
            }
        }
    }

    /// The request precedes remote default/base resolution. Replays use its frozen recovery
    /// fingerprint so a moved remote or dirty retained worktree cannot start another run.
    async fn existing_local_request(
        &self,
        prepared: &PreparedLocalRun,
        digest: &Sha256Digest,
    ) -> Result<Option<RunId>, NativeV2CliError> {
        for run_id in self.local_run_ids()? {
            if !self.recovery_path(&run_id)?.is_file() {
                continue;
            }
            let recovery = self.read_recovery_document(&run_id)?;
            if recovery.submission.submission_key != prepared.submission.submission_key {
                continue;
            }
            let Some(saved) = &recovery.request_digest else {
                continue;
            };
            if saved != digest
                || recovery
                    .invoking_workspace
                    .as_ref()
                    .unwrap_or(&recovery.workspace)
                    != &prepared.workspace
            {
                return Err(NativeV2CliError::SubmissionConflict {
                    existing_run_id: run_id.as_str().to_owned(),
                });
            }
            let final_digest = submission_digest(&recovery.submission).map_err(local_error)?;
            if let Some(run_id) = self
                .matching_submission(run_id, &recovery.submission.submission_key, &final_digest)
                .await?
            {
                return Ok(Some(run_id));
            }
        }
        Ok(None)
    }

    async fn existing_submission(
        &self,
        submission_key: &IdempotencyKey,
        submission_digest: &Sha256Digest,
        workspace: &Path,
    ) -> Result<Option<RunId>, NativeV2CliError> {
        for run_id in self.local_run_ids()? {
            if let Some(existing) = self
                .matching_submission(run_id, submission_key, submission_digest)
                .await?
            {
                let recovery = self.read_recovery_document(&existing)?;
                if recovery
                    .invoking_workspace
                    .as_ref()
                    .unwrap_or(&recovery.workspace)
                    != workspace
                {
                    return Err(NativeV2CliError::SubmissionConflict {
                        existing_run_id: existing.as_str().to_owned(),
                    });
                }
                return Ok(Some(existing));
            }
        }
        Ok(None)
    }

    async fn matching_submission(
        &self,
        run_id: RunId,
        submission_key: &IdempotencyKey,
        submission_digest: &Sha256Digest,
    ) -> Result<Option<RunId>, NativeV2CliError> {
        let ledger_path = self.run_storage(&run_id)?.join("runs.sqlite3");
        if !require_existing_ledger(&ledger_path)? {
            return Ok(None);
        }
        let ledger = SqliteRunLedger::open(&ledger_path).map_err(local_error)?;
        let Some(stored) = ledger
            .get_by_submission_key(submission_key)
            .await
            .map_err(local_error)?
        else {
            return Ok(None);
        };
        if stored.snapshot.run_id != run_id {
            return Err(local_message(
                "run ledger identity does not match its storage",
            ));
        }
        if stored.submission_digest != *submission_digest {
            return Err(NativeV2CliError::SubmissionConflict {
                existing_run_id: run_id.as_str().to_owned(),
            });
        }
        Ok(Some(run_id))
    }

    fn local_run_ids(&self) -> Result<Vec<RunId>, NativeV2CliError> {
        let entries = match std::fs::read_dir(self.state_root.join("runs")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(local_io(error)),
        };
        let mut run_ids = entries
            .map(|entry| entry.map_err(local_io))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(local_run_id_from_entry)
            .collect::<Vec<_>>();
        run_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        Ok(run_ids)
    }

    fn remove_stale_bootstraps(&self, minimum_age: Duration) -> Result<(), NativeV2CliError> {
        for run_id in self.local_run_ids()? {
            let path = self.run_storage(&run_id)?.join(BOOTSTRAP_FILE);
            remove_stale_bootstrap(&path, minimum_age)?;
        }
        Ok(())
    }

    fn workspace_lease(&self, workspace: &Path) -> Result<PathBuf, NativeV2CliError> {
        local_workspace_lease_path(&self.state_root, workspace)
    }

    fn checkpoint_repository(&self, delivery_run_id: &RunId) -> Result<PathBuf, NativeV2CliError> {
        validate_local_run_id(delivery_run_id)?;
        let mut digest = Sha256::new();
        digest.update(b"zeroshot/native-v2/local-checkpoint-repository/v1\0");
        digest.update(delivery_run_id.as_str().as_bytes());
        Ok(self
            .state_root
            .join("checkpoint-repositories")
            .join(format!("{:x}", digest.finalize())))
    }

    fn create_run_storage(
        &self,
        run_id: &openengine_cluster_protocol::RunId,
    ) -> Result<PathBuf, NativeV2CliError> {
        create_local_run_storage(&self.state_root, run_id)
    }

    fn spawn_controller(&self, bootstrap: &Path) -> Result<Child, NativeV2CliError> {
        let mut command = Command::new(&self.executable);
        command
            .arg(LOCAL_CONTROLLER_MODE)
            .arg("--bootstrap")
            .arg(bootstrap)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Harness context crosses the private one-shot bootstrap, not this long-lived process
        // environment. Keep only the platform values needed to re-exec the controller.
        command.env_clear();
        copy_minimal_process_environment(&mut command)?;
        crate::execution::platform::spawn_controller(&mut command).map_err(local_io)
    }

    async fn list_local(&self) -> Result<RunListResult, NativeV2CliError> {
        let runs_root = self.state_root.join("runs");
        let entries = match std::fs::read_dir(&runs_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RunListResult { runs: Vec::new() });
            }
            Err(error) => return Err(local_io(error)),
        };
        let mut runs = Vec::new();
        for entry in entries {
            if let Some(result) = self.list_entry(entry).await? {
                runs.extend(result.runs);
            }
        }
        runs.sort_by(|left, right| left.run_id.as_str().cmp(right.run_id.as_str()));
        Ok(RunListResult { runs })
    }

    async fn list_entry(
        &self,
        entry: std::io::Result<std::fs::DirEntry>,
    ) -> Result<Option<RunListResult>, NativeV2CliError> {
        let Some(run_id) = entry.ok().and_then(local_run_id_from_entry) else {
            return Ok(None);
        };
        let result = match self.list_entry_once(&run_id).await {
            Ok(result) => Ok(result),
            Err(_) => {
                sleep(CONTROLLER_HANDOFF_RETRY_DELAY).await;
                self.list_entry_once(&run_id).await
            }
        };
        match result {
            Ok(result) => Ok(Some(result)),
            Err(NativeV2CliError::Local(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn list_entry_once(&self, run_id: &RunId) -> Result<RunListResult, NativeV2CliError> {
        let result = self
            .status_local(RunStatusParams {
                run_id: run_id.clone(),
            })
            .await?;
        Ok(RunListResult { runs: vec![result] })
    }

    async fn status_local(
        &self,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, NativeV2CliError> {
        let retry = params.clone();
        match self.status_once(params).await {
            Ok(mut result) => {
                result.workspace_recovery = self.workspace_recovery(&result).await?;
                Ok(result)
            }
            Err(_) => {
                sleep(CONTROLLER_HANDOFF_RETRY_DELAY).await;
                let mut result = self.status_once(retry).await?;
                result.workspace_recovery = self.workspace_recovery(&result).await?;
                Ok(result)
            }
        }
    }

    fn recovery_path(&self, run_id: &RunId) -> Result<PathBuf, NativeV2CliError> {
        Ok(self.run_storage(run_id)?.join(RECOVERY_FILE))
    }

    fn recovery_claim_path(&self, run_id: &RunId) -> Result<PathBuf, NativeV2CliError> {
        Ok(self.run_storage(run_id)?.join(RECOVERY_CLAIM_FILE))
    }

    fn claim_recovery_workspace(
        &self,
        run_id: &RunId,
    ) -> Result<LocalRecoveryClaim, NativeV2CliError> {
        let claim = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.recovery_claim_path(run_id)?)
            .map_err(local_io)?;
        claim.try_lock_exclusive().map_err(|error| {
            if error.kind() == fs2::lock_contended_error().kind() {
                local_message("retained workspace is already being resumed")
            } else {
                local_io(error)
            }
        })?;
        Ok(LocalRecoveryClaim(claim))
    }

    fn recovery_workspace_is_unclaimed(&self, run_id: &RunId) -> Result<bool, NativeV2CliError> {
        let path = self.recovery_claim_path(run_id)?;
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            _ => Ok(ControllerLease::probe(&path).is_ok_and(|probe| !probe.held)),
        }
    }

    /// Releases a successor claim whose controller never admitted the run. `handoff` bounds the
    /// wait for a launcher that may still be starting that controller; it is zero once the
    /// controller is known to have stopped. A pending restore stays for the next attempt.
    async fn reconcile_local_resume_claim(
        &self,
        run_id: &RunId,
        handoff: Duration,
    ) -> Result<(), NativeV2CliError> {
        let Some((mut recovery, successor_run_id, successor_storage)) =
            self.claimed_local_successor(run_id)?
        else {
            return Ok(());
        };
        if self
            .successor_claim_is_live_or_admitted(&successor_run_id, &successor_storage, handoff)
            .await?
        {
            return Ok(());
        }
        std::fs::remove_dir_all(&successor_storage).map_err(local_io)?;
        recovery.successor_run_id = None;
        self.write_recovery_document(run_id, &recovery)
    }

    async fn successor_claim_is_live_or_admitted(
        &self,
        successor_run_id: &RunId,
        successor_storage: &Path,
        handoff: Duration,
    ) -> Result<bool, NativeV2CliError> {
        let paths = self.paths(successor_run_id)?;
        let ledger_path = successor_storage.join("runs.sqlite3");
        let deadline = Instant::now() + handoff;
        loop {
            match ControllerLease::acquire(paths.lease()) {
                Err(ControllerLeaseError::Held) => return Ok(true),
                Ok(lease) => {
                    if self
                        .local_successor_was_admitted(&ledger_path, successor_run_id)
                        .await?
                    {
                        return Ok(true);
                    }
                    if Instant::now() >= deadline {
                        drop(lease);
                        return Ok(false);
                    }
                    drop(lease);
                    sleep(CONTROLLER_HANDOFF_RETRY_DELAY).await;
                }
                Err(error) => return Err(local_error(error)),
            }
        }
    }

    fn claimed_local_successor(
        &self,
        run_id: &RunId,
    ) -> Result<Option<(LocalRecoveryDocument, RunId, PathBuf)>, NativeV2CliError> {
        let mut recovery = self.read_recovery_document(run_id)?;
        let Some(successor_run_id) = recovery.successor_run_id.clone() else {
            return Ok(None);
        };
        let successor_storage = self.run_storage(&successor_run_id)?;
        if successor_storage.exists() {
            return Ok(Some((recovery, successor_run_id, successor_storage)));
        }
        recovery.successor_run_id = None;
        self.write_recovery_document(run_id, &recovery)?;
        Ok(None)
    }

    async fn local_successor_was_admitted(
        &self,
        ledger_path: &Path,
        successor_run_id: &RunId,
    ) -> Result<bool, NativeV2CliError> {
        if !require_existing_ledger(ledger_path)? {
            return Ok(false);
        }
        let ledger = SqliteRunLedger::open_read_only(ledger_path).map_err(local_error)?;
        Ok(ledger
            .get(successor_run_id)
            .await
            .map_err(local_error)?
            .is_some())
    }

    fn read_recovery_document(
        &self,
        run_id: &RunId,
    ) -> Result<LocalRecoveryDocument, NativeV2CliError> {
        LocalRecoveryDocument::read(&self.recovery_path(run_id)?)
    }

    fn write_recovery_document(
        &self,
        run_id: &RunId,
        document: &LocalRecoveryDocument,
    ) -> Result<(), NativeV2CliError> {
        document.write(&self.recovery_path(run_id)?)
    }
}

impl LocalRecoveryDocument {
    fn read(path: &Path) -> Result<Self, NativeV2CliError> {
        let bytes = std::fs::read(path).map_err(local_io)?;
        serde_json::from_slice(&bytes)
            .map_err(|_| local_message("workspace recovery metadata is invalid"))
    }

    fn write(&self, path: &Path) -> Result<(), NativeV2CliError> {
        let parent = path
            .parent()
            .ok_or_else(|| local_message("workspace recovery path is invalid"))?;
        let temporary = parent.join(format!(".workspace-recovery-{}.tmp", uuid::Uuid::now_v7()));
        let bytes = serde_json::to_vec(self)
            .map_err(|_| local_message("workspace recovery metadata could not be encoded"))?;
        let result = (|| {
            let file = private_file(&temporary, FileAccess::CreateNew).map_err(local_io)?;
            write_and_commit(
                file,
                &bytes,
                CommitPaths {
                    temporary: &temporary,
                    destination: path,
                    parent,
                },
            )
        })();
        cleanup_temporary(result, &temporary)
    }
}

impl LocalCliBackend {
    fn delivery_run_id(
        &self,
        run_id: &RunId,
        document: &LocalRecoveryDocument,
    ) -> Result<RunId, NativeV2CliError> {
        let mut lineage_run_id = run_id.clone();
        let mut lineage = document.clone();
        for _ in 0..MAX_RECOVERY_LINEAGE_DEPTH {
            if let Some(delivery_run_id) = lineage.delivery_run_id {
                return Ok(delivery_run_id);
            }
            let Some(predecessor) = lineage.resumed_from else {
                return Ok(lineage_run_id);
            };
            lineage_run_id = predecessor;
            lineage = self.read_recovery_document(&lineage_run_id)?;
        }
        Err(local_message("workspace recovery lineage is too deep"))
    }

    async fn workspace_recovery(
        &self,
        status: &RunStatusResult,
    ) -> Result<WorkspaceRecovery, NativeV2CliError> {
        let document = match self.read_recovery_document(&status.run_id) {
            Ok(document) => document,
            Err(_) => return Ok(WorkspaceRecovery::default()),
        };
        let failed = matches!(
            status.status,
            openengine_cluster_protocol::RunStatus::Finished {
                terminal_result: TerminalResult::Failed { .. },
                ..
            }
        );
        let loss_observed = status.controller.as_ref().is_some_and(|observation| {
            observation.state == openengine_cluster_protocol::ControllerState::Lost
        });
        let unclaimed = (failed || loss_observed)
            && document.workspace.is_dir()
            && self.recovery_workspace_is_unclaimed(&status.run_id)?;
        // With the recovery claim released, a successor that neither runs nor was admitted is
        // an interrupted launch, such as a crash during its restore. The next resume reconciles
        // it before claiming the workspace, so it does not block recovery.
        let successor_run_id = match document.successor_run_id {
            Some(successor) if unclaimed && self.interrupted_launch(&successor).await => None,
            successor => successor,
        };
        Ok(WorkspaceRecovery {
            recoverable: unclaimed && successor_run_id.is_none(),
            connection_requirements: document
                .submission
                .connection_requirements()
                .into_iter()
                .map(|(key, fields)| (key, fields.into_iter().collect()))
                .collect(),
            resumed_from: document.resumed_from,
            successor_run_id,
        })
    }

    async fn interrupted_launch(&self, successor_run_id: &RunId) -> bool {
        let Ok(storage) = self.run_storage(successor_run_id) else {
            return false;
        };
        if !storage.exists() {
            return true;
        }
        let Ok(paths) = self.paths(successor_run_id) else {
            return false;
        };
        let released = match std::fs::symlink_metadata(paths.lease()) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            _ => ControllerLease::probe(&paths.lease()).is_ok_and(|probe| !probe.held),
        };
        released
            && matches!(
                self.local_successor_was_admitted(&storage.join("runs.sqlite3"), successor_run_id)
                    .await,
                Ok(false)
            )
    }

    async fn status_once(
        &self,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, NativeV2CliError> {
        self.inspect_status(params).await
    }

    async fn force_local(
        &self,
        params: RunForceParams,
    ) -> Result<RunForceResult, NativeV2CliError> {
        let run_id = params.run_id.clone();
        let retry = params.clone();
        match self.force_once(params).await {
            Ok(_) => {}
            Err(_) => {
                sleep(CONTROLLER_HANDOFF_RETRY_DELAY).await;
                self.force_once(retry).await?;
            }
        };
        let status = self.status_local(RunStatusParams { run_id }).await?;
        Ok(RunForceResult {
            run_id: status.run_id,
            title: status.title,
            source: status.source,
            size: status.size,
            at_cursor: status.at_cursor,
            status: status.status,
            controller: None,
            workspace_recovery: status.workspace_recovery,
        })
    }

    async fn force_once(&self, params: RunForceParams) -> Result<RunForceResult, NativeV2CliError> {
        let recorded = self
            .read_recorded_status(RunStatusParams {
                run_id: params.run_id.clone(),
            })
            .await;
        if let Ok(recorded) = recorded
            && matches!(
                recorded.status,
                openengine_cluster_protocol::RunStatus::Finished { .. }
            )
        {
            return Ok(RunForceResult {
                run_id: recorded.run_id,
                title: recorded.title,
                source: recorded.source,
                size: recorded.size,
                at_cursor: recorded.at_cursor,
                status: recorded.status,
                controller: None,
                workspace_recovery: recorded.workspace_recovery,
            });
        }
        let transport = self.connect_run(&params.run_id).await?;
        ClusterClient::new(transport.as_ref())
            .run_force(params)
            .await
            .map_err(protocol_error)
    }
}

fn remove_stale_bootstrap(path: &Path, minimum_age: Duration) -> Result<(), NativeV2CliError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(());
            }
            return Err(local_io(error));
        }
    };
    let stale = metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= minimum_age);
    if stale {
        std::fs::remove_file(path).map_err(local_io)?;
    }
    Ok(())
}

pub(crate) fn local_workspace_lease_path(
    state_root: &Path,
    workspace: &Path,
) -> Result<PathBuf, NativeV2CliError> {
    let root = state_root.join("workspaces");
    prepare_private_directory(&root)?;
    Ok(crate::native_v2_portable_controller::workspace_lease_path(
        &root, workspace,
    ))
}

pub(crate) fn create_local_run_storage(
    state_root: &Path,
    run_id: &RunId,
) -> Result<PathBuf, NativeV2CliError> {
    validate_local_run_id(run_id)?;
    let runs = state_root.join("runs");
    prepare_private_directory(&runs)?;
    let storage = runs.join(run_id.as_str());
    crate::execution::platform::create_private_directory(&storage).map_err(local_io)?;
    Ok(storage)
}

async fn wait_for_controller(
    child: &mut Child,
    paths: &PortableControllerPaths,
    run_id: &openengine_cluster_protocol::RunId,
    timeout: Duration,
) -> Result<ControllerWait, NativeV2CliError> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(ready) = read_ready(paths) {
            if &ready.run_id == run_id {
                // Windows readiness can be visible while no named-pipe instance is currently
                // available. Keep the connection in the existing bounded readiness loop.
                if connect_transport(paths).await.is_ok() {
                    return Ok(ControllerWait::Ready);
                }
            }
        }
        if child.try_wait().map_err(local_io)?.is_some() {
            return Ok(ControllerWait::Exited(take_startup_failure(paths)));
        }
        if Instant::now() >= deadline {
            return Ok(ControllerWait::TimedOut);
        }
        sleep(Duration::from_millis(20)).await;
    }
}

/// Reads the finite preparation deadline override. Empty means unset, like other local settings.
fn preparation_timeout(value: Option<std::ffi::OsString>) -> Result<Duration, NativeV2CliError> {
    let Some(value) = value else {
        return Ok(DEFAULT_PREPARATION_TIMEOUT);
    };
    value
        .to_str()
        .filter(|seconds| seconds.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .filter(|seconds| (1..=MAX_PREPARATION_TIMEOUT_SECONDS).contains(seconds))
        .map(Duration::from_secs)
        .ok_or_else(|| {
            NativeV2CliError::Usage(format!(
                "{PREPARATION_TIMEOUT_ENV} must be a whole number of seconds from 1 to {MAX_PREPARATION_TIMEOUT_SECONDS}"
            ))
        })
}

fn require_local(target: Option<&str>) -> Result<(), NativeV2CliError> {
    if target.is_none() {
        Ok(())
    } else {
        Err(local_message("local backend cannot serve a named target"))
    }
}

fn protocol_error(error: ClientError) -> NativeV2CliError {
    super::diagnostic::client_error(error)
}

fn local_error(error: impl std::fmt::Display) -> NativeV2CliError {
    local_message(error.to_string())
}

fn local_io(error: std::io::Error) -> NativeV2CliError {
    local_message(error.to_string())
}

fn local_message(message: impl Into<String>) -> NativeV2CliError {
    NativeV2CliError::Local(message.into())
}

#[cfg(test)]
#[path = "local/recovery_claim_tests.rs"]
mod recovery_claim_tests;

#[cfg(test)]
#[path = "local/tests.rs"]
mod local_contract_tests;

fn checked_review_token(value: String) -> Result<String, NativeV2CliError> {
    if value.is_empty() || value.len() > 4096 || value.contains('\0') {
        return Err(local_message("GitHub credential is invalid"));
    }
    Ok(value)
}

fn review_environment_token() -> Result<Option<String>, NativeV2CliError> {
    for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Some(value) = std::env::var_os(name) {
            let value = value
                .into_string()
                .map_err(|_| local_message("GitHub credential must be UTF-8"))?;
            return checked_review_token(value).map(Some);
        }
    }
    Ok(None)
}

async fn review_token(directory: &Path) -> Result<String, NativeV2CliError> {
    if let Some(token) = review_environment_token()? {
        return Ok(token);
    }
    let environment =
        crate::native_v2_local::capture_local_native_environment(directory).map_err(local_error)?;
    let executable = crate::execution::platform::find_executable("gh", &environment, directory)
        .ok_or_else(|| local_message("review resolution requires GitHub CLI on PATH"))?;
    let mut command = Command::new(executable);
    command
        .env_clear()
        .current_dir(directory)
        .args(["auth", "token", "--hostname", "github.com"]);
    for name in [
        "HOME",
        "PATH",
        "GH_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        "APPDATA",
        "USERPROFILE",
        "SystemRoot",
    ] {
        if let Some(value) = environment.get(name) {
            command.env(name, value);
        }
    }
    let result = crate::native_v2_delivery::command::capture_sensitive(
        &mut command,
        Duration::from_secs(10),
        &[],
    )
    .await
    .map_err(|_| {
        local_message("GitHub login is unavailable; sign in with gh or supply GH_TOKEN")
    })?;
    if result.stdout_truncated() || result.diagnostic().exit_status != Some(0) {
        return Err(local_message(
            "GitHub login is unavailable; sign in with gh or supply GH_TOKEN",
        ));
    }
    checked_review_token(result.stdout().trim().to_owned())
}
