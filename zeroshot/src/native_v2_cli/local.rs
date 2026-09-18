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
use crate::execution::platform::ControllerChild as Child;
use tokio::time::{Instant, sleep};

use super::oecp::{ChannelSubscription, spawn_attach, spawn_logs, spawn_watch};
use super::{
    CliRunForceResult, CliRunListResult, CliRunStatusResult, CliRunWatchEventNotification,
    LocalRunProfileStore, NativeV2CliBackend, NativeV2CliError, PreparedRunRequest, TargetAdd,
    default_local_state_root,
};
use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::native_v2_cloud::submission_digest;
use crate::native_v2_local::{PreparedLocalRun, prepare_local_run};
use crate::native_v2_portable_controller::{
    ControllerLease, ControllerLeaseError, PortableControllerBootstrap, PortableControllerError,
    PortableControllerPaths, PortableRunController, read_ready, write_bootstrap_file,
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
    delivery_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resumed_from: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    successor_run_id: Option<RunId>,
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
        Ok(Self {
            state_root: default_local_state_root()?,
            executable: std::env::current_exe().map_err(local_io)?,
            current_directory: std::env::current_dir().map_err(local_io)?,
            git_program: PathBuf::from("git"),
            ready_timeout: DEFAULT_READY_TIMEOUT,
        })
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

            match PortableRunController::open_observer(paths.clone(), run_id.clone()).await {
                Ok(observer) => {
                    let observer = Arc::new(observer);
                    let server = observer.bind().await.map_err(local_error)?;
                    tokio::spawn(async move {
                        let _ = server.serve().await;
                    });
                    return connect_transport(&paths).await.map_err(local_error);
                }
                Err(PortableControllerError::Lease(ControllerLeaseError::Held))
                    if Instant::now() < deadline =>
                {
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
        self.start_prepared_controller_with_lineage(prepared, None)
            .await
    }

    async fn start_prepared_controller_with_lineage(
        &self,
        prepared: PreparedLocalRun,
        resumed_from: Option<RunId>,
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
        let bootstrap_path = storage.join(BOOTSTRAP_FILE);
        let bootstrap = PortableControllerBootstrap {
            run_id: prepared.run_id.clone(),
            delivery_run_id: prepared.delivery_run_id,
            adopt_existing_delivery,
            submission: prepared.submission,
            environment: prepared.environment,
            github_token: prepared.github_token,
            workspace: prepared.workspace,
            workspace_lease,
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

    fn workspace_lease(&self, workspace: &Path) -> Result<PathBuf, NativeV2CliError> {
        let root = self.state_root.join("workspaces");
        prepare_private_directory(&root)?;
        let digest = Sha256::digest(workspace.as_os_str().as_encoded_bytes());
        Ok(root.join(format!("{digest:x}.lock")))
    }

    fn create_run_storage(
        &self,
        run_id: &openengine_cluster_protocol::RunId,
    ) -> Result<PathBuf, NativeV2CliError> {
        let runs = self.state_root.join("runs");
        prepare_private_directory(&runs)?;
        let storage = self.run_storage(run_id)?;
        crate::execution::platform::create_private_directory(&storage).map_err(local_io)?;
        Ok(storage)
    }

    fn spawn_controller(&self, bootstrap: &Path) -> Result<Child, NativeV2CliError> {
        let mut command = Command::new(&self.executable);
        command
            .arg(LOCAL_CONTROLLER_MODE)
            .arg("--bootstrap")
            .arg(bootstrap)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_clear();
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

    fn claim_recovery_workspace(&self, run_id: &RunId) -> Result<File, NativeV2CliError> {
        let claim = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.recovery_claim_path(run_id)?)
            .map_err(local_io)?;
        claim.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                local_message("retained workspace is already being resumed")
            } else {
                local_io(error)
            }
        })?;
        Ok(claim)
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
        let mut recovery = self.read_recovery_document(run_id)?;
        let Some(successor_run_id) = recovery.successor_run_id.clone() else {
            return Ok(());
        };
        let successor_storage = self.run_storage(&successor_run_id)?;
        if !successor_storage.exists() {
            recovery.successor_run_id = None;
            return self.write_recovery_document(run_id, &recovery);
        }
        let paths = self.paths(&successor_run_id)?;
        let ledger_path = successor_storage.join("runs.sqlite3");
        let deadline = Instant::now() + self.ready_timeout;
        loop {
            if require_existing_ledger(&ledger_path)? || read_ready(&paths).is_ok() {
                return Ok(());
            }
            match ControllerLease::acquire(paths.lease()) {
                Err(ControllerLeaseError::Held) => return Ok(()),
                Ok(lease) if Instant::now() >= deadline => {
                    std::fs::remove_dir_all(&successor_storage).map_err(local_io)?;
                    drop(lease);
                    recovery.successor_run_id = None;
                    return self.write_recovery_document(run_id, &recovery);
                }
                Ok(lease) => {
                    drop(lease);
                    sleep(CONTROLLER_HANDOFF_RETRY_DELAY).await;
                }
                Err(error) => return Err(local_error(error)),
            }
        }
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
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(document)
            .map_err(|_| local_message("workspace recovery metadata could not be encoded"))?;
        std::fs::write(&temporary, bytes).map_err(local_io)?;
        std::fs::rename(temporary, path).map_err(local_io)
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
        let retry = params.clone();
        match self.force_once(params).await {
            Ok(result) => Ok(result),
            Err(_) => {
                sleep(CONTROLLER_HANDOFF_RETRY_DELAY).await;
                self.force_once(retry).await
            }
        }
    }

    async fn force_once(&self, params: RunForceParams) -> Result<RunForceResult, NativeV2CliError> {
        let transport = self.connect_run(&params.run_id).await?;
        ClusterClient::new(transport.as_ref())
            .run_force(params)
            .await
            .map_err(protocol_error)
    }
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
                connect_transport(paths).await.map_err(local_error)?;
                return Ok(());
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
mod recovery_claim_tests {
    use super::*;
    use openengine_cluster_testkit::assertions::AssertValue;

    #[test]
    fn interrupted_resume_releases_the_workspace_claim_for_retry() {
        let root =
            std::env::temp_dir().join(format!("zeroshot-local-recovery-{}", uuid::Uuid::now_v7()));
        let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000001");
        std::fs::create_dir_all(root.join("runs").join(run_id.as_str())).assert_value();
        let backend = LocalCliBackend::new(
            root.clone(),
            PathBuf::from("zeroshot"),
            root.clone(),
            PathBuf::from("git"),
        );

        let interrupted_process_claim = backend.claim_recovery_workspace(&run_id).assert_value();
        assert!(
            !backend
                .recovery_workspace_is_unclaimed(&run_id)
                .assert_value()
        );

        drop(interrupted_process_claim);
        assert!(
            backend
                .recovery_workspace_is_unclaimed(&run_id)
                .assert_value()
        );

        std::fs::remove_dir_all(root).assert_value();
    }
}
