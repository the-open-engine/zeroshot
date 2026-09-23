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
use super::support::{CommitPaths, cleanup_temporary, write_and_commit};
use super::{
    CliRunForceResult, CliRunListResult, CliRunStatusResult, CliRunWatchEventNotification,
    LocalRunProfileStore, NativeV2CliBackend, NativeV2CliError, PreparedRunRequest, TargetAdd,
    default_local_state_root,
};
use crate::execution::platform::{ControllerChild as Child, FileAccess, private_file};
use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::native_v2_cloud::submission_digest;
use crate::native_v2_local::{PreparedLocalRun, prepare_local_run};
use crate::native_v2_portable_controller::{
    ControllerLease, ControllerLeaseError, PortableControllerBootstrap, PortableControllerError,
    PortableControllerPaths, PortableControllerServer, PortableRunController, read_ready,
    write_bootstrap_file,
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

/// Private process mode intercepted by the shipped binary before public CLI parsing.
#[doc(hidden)]
pub const LOCAL_CONTROLLER_MODE: &str = "__zeroshot-run-controller";

const BOOTSTRAP_FILE: &str = "controller.bootstrap.json";
const SUBMISSION_LOCK_FILE: &str = "submission.lock";
const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(10);
const STALE_BOOTSTRAP_AGE: Duration = Duration::from_secs(60);
const CONTROLLER_HANDOFF_RETRY_DELAY: Duration = Duration::from_millis(25);
const RECOVERY_FILE: &str = "workspace-recovery.json";
const RECOVERY_CLAIM_FILE: &str = "workspace-recovery.claim";
const MAX_RECOVERY_LINEAGE_DEPTH: usize = 64;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LocalRecoveryDocument {
    submission: RunSubmission,
    workspace: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resumed_from: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    successor_run_id: Option<RunId>,
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
}

impl LocalCliBackend {
    pub fn production() -> Result<Self, NativeV2CliError> {
        let backend = Self {
            state_root: default_local_state_root()?,
            executable: std::env::current_exe().map_err(local_io)?,
            current_directory: std::env::current_dir().map_err(local_io)?,
            git_program: PathBuf::from("git"),
            ready_timeout: DEFAULT_READY_TIMEOUT,
        };
        backend.remove_stale_bootstraps(STALE_BOOTSTRAP_AGE)?;
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
        }
    }

    #[must_use]
    pub fn with_ready_timeout(mut self, timeout: Duration) -> Self {
        self.ready_timeout = timeout;
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

    async fn connect_run(
        &self,
        run_id: &openengine_cluster_protocol::RunId,
    ) -> Result<Arc<PortableControllerTransport>, NativeV2CliError> {
        let paths = self.existing_run_paths(run_id)?;
        let deadline = Instant::now() + self.ready_timeout;
        loop {
            if read_ready(&paths).is_ok_and(|ready| &ready.run_id != run_id) {
                return Err(local_message(
                    "controller readiness has a different run identity",
                ));
            }
            if let Ok(transport) = connect_transport(&paths).await {
                return Ok(transport);
            }

            match bind_observer(paths.clone(), run_id.clone()).await {
                Ok(server) => {
                    tokio::spawn(async move {
                        let _ = server.serve().await;
                    });
                    // The observer owns the lease now; reconnect through the same bounded loop so
                    // Windows does not fail the handoff on one unavailable named-pipe instance.
                }
                Err(error) if retryable_controller_handoff(&error) && Instant::now() < deadline => {
                    sleep(CONTROLLER_HANDOFF_RETRY_DELAY).await;
                }
                Err(error) => return Err(local_error(error)),
            }
        }
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
        mut request: PreparedRunRequest,
    ) -> Result<openengine_cluster_protocol::RunId, NativeV2CliError> {
        NativeV2Admission
            .validate_intent(&request.intent, DeliveryPolicy::Optional)
            .await
            .map_err(NativeV2CliError::InvalidRun)?;
        request.connections = LocalConnectionStore::new(self.state_root.clone())
            .resolve(&request.intent.runtime, &request.connections)?
            .bootstrap_values();
        let prepared = prepare_local_run(request, &self.current_directory, &self.git_program)
            .map_err(local_error)?;
        let digest = submission_digest(&prepared.submission).map_err(local_error)?;
        let _submission_lock = self.acquire_submission_lock().await?;
        if let Some(run_id) = self
            .existing_submission(&prepared.submission.submission_key, &digest)
            .await?
        {
            return Ok(run_id);
        }
        self.start_prepared_controller(prepared).await
    }

    async fn start_prepared_controller(
        &self,
        prepared: PreparedLocalRun,
    ) -> Result<RunId, NativeV2CliError> {
        self.start_prepared_controller_with_lineage(prepared, None, None)
            .await
    }

    async fn start_prepared_controller_with_lineage(
        &self,
        prepared: PreparedLocalRun,
        resumed_from: Option<RunId>,
        checkpoint: Option<crate::native_v2_supervisor::checkpoints::CheckpointRestore>,
    ) -> Result<RunId, NativeV2CliError> {
        let adopt_existing_delivery = resumed_from.is_some();
        let paths = self.paths(&prepared.run_id)?;
        let storage = self.create_run_storage(&prepared.run_id)?;
        self.write_recovery_document(
            &prepared.run_id,
            &LocalRecoveryDocument {
                submission: prepared.submission.clone(),
                workspace: prepared.workspace.clone(),
                delivery_run_id: Some(prepared.delivery_run_id.clone()),
                resumed_from,
                successor_run_id: None,
            },
        )?;
        let workspace_lease = self.workspace_lease(&prepared.workspace)?;
        let checkpoint_repository = self.checkpoint_repository(&prepared.delivery_run_id)?;
        let bootstrap_path = storage.join(BOOTSTRAP_FILE);
        let bootstrap = PortableControllerBootstrap {
            checkpoint,
            run_id: prepared.run_id.clone(),
            delivery_run_id: prepared.delivery_run_id,
            adopt_existing_delivery,
            submission: prepared.submission,
            environment: prepared.environment,
            native_environment: prepared.native_environment,
            github_token: prepared.github_token,
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
                return Err(error);
            }
        };
        if let Err(error) =
            wait_for_controller(&mut child, &paths, &prepared.run_id, self.ready_timeout).await
        {
            let _ = child.kill().await;
            remove_private_bootstrap(&bootstrap_path);
            return Err(error);
        }
        Ok(prepared.run_id)
    }

    async fn acquire_submission_lock(&self) -> Result<ControllerLease, NativeV2CliError> {
        let path = self.state_root.join(SUBMISSION_LOCK_FILE);
        loop {
            match ControllerLease::acquire(&path) {
                Ok(lock) => return Ok(lock),
                Err(ControllerLeaseError::Held) => sleep(Duration::from_millis(20)).await,
                Err(error) => return Err(local_error(error)),
            }
        }
    }

    async fn existing_submission(
        &self,
        submission_key: &IdempotencyKey,
        submission_digest: &Sha256Digest,
    ) -> Result<Option<RunId>, NativeV2CliError> {
        for run_id in self.local_run_ids()? {
            if let Some(existing) = self
                .matching_submission(run_id, submission_key, submission_digest)
                .await?
            {
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
        let transport = self.connect_run(run_id).await?;
        let mut result = ClusterClient::new(transport.as_ref())
            .run_list(RunListParams::default())
            .await
            .map_err(protocol_error)?;
        for status in &mut result.runs {
            status.workspace_recovery = self.workspace_recovery(status)?;
        }
        Ok(result)
    }

    async fn status_local(
        &self,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, NativeV2CliError> {
        let retry = params.clone();
        match self.status_once(params).await {
            Ok(mut result) => {
                result.workspace_recovery = self.workspace_recovery(&result)?;
                Ok(result)
            }
            Err(_) => {
                sleep(CONTROLLER_HANDOFF_RETRY_DELAY).await;
                let mut result = self.status_once(retry).await?;
                result.workspace_recovery = self.workspace_recovery(&result)?;
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
        match self.claim_recovery_workspace(run_id) {
            Ok(claim) => {
                drop(claim);
                Ok(true)
            }
            Err(NativeV2CliError::Local(message))
                if message == "retained workspace is already being resumed" =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    async fn reconcile_local_resume_claim(&self, run_id: &RunId) -> Result<(), NativeV2CliError> {
        let Some((mut recovery, successor_run_id, successor_storage)) =
            self.claimed_local_successor(run_id)?
        else {
            return Ok(());
        };
        if self
            .successor_claim_is_live_or_admitted(&successor_run_id, &successor_storage)
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
    ) -> Result<bool, NativeV2CliError> {
        let paths = self.paths(successor_run_id)?;
        let ledger_path = successor_storage.join("runs.sqlite3");
        let deadline = Instant::now() + self.ready_timeout;
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
        let bytes = std::fs::read(self.recovery_path(run_id)?).map_err(local_io)?;
        serde_json::from_slice(&bytes)
            .map_err(|_| local_message("workspace recovery metadata is invalid"))
    }

    fn write_recovery_document(
        &self,
        run_id: &RunId,
        document: &LocalRecoveryDocument,
    ) -> Result<(), NativeV2CliError> {
        let path = self.recovery_path(run_id)?;
        let parent = path
            .parent()
            .ok_or_else(|| local_message("workspace recovery path is invalid"))?;
        let temporary = parent.join(format!(".workspace-recovery-{}.tmp", uuid::Uuid::now_v7()));
        let bytes = serde_json::to_vec(document)
            .map_err(|_| local_message("workspace recovery metadata could not be encoded"))?;
        let result = (|| {
            let file = private_file(&temporary, FileAccess::CreateNew).map_err(local_io)?;
            write_and_commit(
                file,
                &bytes,
                CommitPaths {
                    temporary: &temporary,
                    destination: &path,
                    parent,
                },
            )
        })();
        cleanup_temporary(result, &temporary)
    }

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

    fn workspace_recovery(
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
        Ok(WorkspaceRecovery {
            recoverable: failed
                && document.successor_run_id.is_none()
                && document.workspace.is_dir()
                && self.recovery_workspace_is_unclaimed(&status.run_id)?,
            connection_requirements: document
                .submission
                .runtime
                .connection_requirements()
                .into_iter()
                .map(|(key, fields)| (key, fields.into_iter().collect()))
                .collect(),
            resumed_from: document.resumed_from,
            successor_run_id: document.successor_run_id,
        })
    }

    async fn status_once(
        &self,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, NativeV2CliError> {
        let transport = self.connect_run(&params.run_id).await?;
        ClusterClient::new(transport.as_ref())
            .run_status(params)
            .await
            .map_err(protocol_error)
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
            workspace_recovery: status.workspace_recovery,
        })
    }

    async fn force_once(&self, params: RunForceParams) -> Result<RunForceResult, NativeV2CliError> {
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
    let digest = Sha256::digest(workspace.as_os_str().as_encoded_bytes());
    Ok(root.join(format!("{digest:x}.lock")))
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
) -> Result<(), NativeV2CliError> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(ready) = read_ready(paths) {
            if &ready.run_id == run_id {
                // Windows readiness can be visible while no named-pipe instance is currently
                // available. Keep the connection in the existing bounded readiness loop.
                if connect_transport(paths).await.is_ok() {
                    return Ok(());
                }
            }
        }
        if child.try_wait().map_err(local_io)?.is_some() {
            return Err(local_message("controller exited before becoming ready"));
        }
        if Instant::now() >= deadline {
            return Err(local_message(
                "controller did not become ready before the deadline",
            ));
        }
        sleep(Duration::from_millis(20)).await;
    }
}

async fn bind_observer(
    paths: PortableControllerPaths,
    run_id: RunId,
) -> Result<PortableControllerServer, PortableControllerError> {
    Arc::new(PortableRunController::open_observer(paths, run_id).await?)
        .bind()
        .await
}

fn retryable_controller_handoff(error: &PortableControllerError) -> bool {
    matches!(
        error,
        PortableControllerError::Lease(ControllerLeaseError::Held) | PortableControllerError::Io(_)
    )
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
