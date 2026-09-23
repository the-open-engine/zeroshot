//! Experimental ACP facade for one reusable local Zeroshot profile.
//!
//! Each ACP prompt is an ordinary durable native-v2 run. The outer ACP session alone owns the
//! workspace lease, provider runtime home, and node-instance provider sessions reused by turns.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as SyncMutex, Weak};
use std::time::Duration;

use agent_client_protocol as acp;
use acp::Client as _;
use async_trait::async_trait;
use openengine_cluster_protocol::{
    FieldName, GraphNode, IdempotencyKey, PayloadType, RunConnectionValues, RunId, RunProfile,
    RunProfileName, RunProfileScope, RunProfileSelector, RunSubmission, RunTitle, RuntimePlan,
    SessionScope, TerminalResult,
};
use serde_json::{json, Map, Value};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission, NativeV2AdmissionError};
use crate::native_v2_candidate::{ProviderAccessPlacement, materialize_provider_access};
use crate::native_v2_cli::local::{create_local_run_storage, local_workspace_lease_path};
use crate::native_v2_cli::{default_local_state_root, LocalRunProfileStore, NativeV2CliError};
use crate::native_v2_cloud::{submission_digest, NativeV2CloudError};
use crate::native_v2_contract::NodeRuntimeBinding;
use crate::native_v2_local::{
    build_local_owner_process_candidate, local_resolved_source, LocalCompositionError,
    LocalProcessCandidateRequest,
};
use crate::native_v2_portable_controller::{
    ControllerLease, ControllerLeaseError, PortableControllerError, PortableControllerPaths,
    WorkspaceIdentity,
};
use crate::native_v2_runner::NativeNodeRunner;
use crate::native_v2_supervisor::{
    NativeV2Supervisor, NativeV2SupervisorError, RunEnvironment, RunEnvironmentError,
};
use crate::v2_run_ledger::sqlite::SqliteRunLedger;
use crate::v2_run_ledger::{CreateRun, RunLedger, RunLedgerError};

const ACP_ERROR_INVALID_PARAMS: i32 = -32602;
const ACP_ERROR_INTERNAL: i32 = -32603;
const WORKSPACE_MONITOR_INTERVAL: Duration = Duration::from_millis(100);

type SessionUpdate = (acp::SessionNotification, oneshot::Sender<Result<(), ()>>);

#[derive(Debug, Error)]
pub enum AcpServeError {
    #[error(transparent)]
    Cli(#[from] NativeV2CliError),
    #[error(transparent)]
    Admission(#[from] NativeV2AdmissionError),
    #[error(transparent)]
    Composition(#[from] LocalCompositionError),
    #[error(transparent)]
    ControllerLease(#[from] ControllerLeaseError),
    #[error(transparent)]
    Portable(#[from] PortableControllerError),
    #[error(transparent)]
    Environment(#[from] RunEnvironmentError),
    #[error(transparent)]
    Cloud(#[from] NativeV2CloudError),
    #[error(transparent)]
    Ledger(#[from] RunLedgerError),
    #[error(transparent)]
    Supervisor(#[from] NativeV2SupervisorError),
    #[error("local ACP storage could not be prepared")]
    Storage(#[source] std::io::Error),
    #[error("ACP profile is not eligible: {0}")]
    Profile(&'static str),
    #[error("ACP request is invalid: {0}")]
    Request(&'static str),
    #[error("ACP transport failed: {0}")]
    Transport(String),
}

impl AcpServeError {
    fn rpc_error(&self) -> acp::Error {
        match self {
            Self::Profile(message) | Self::Request(message) => {
                acp::Error::new(ACP_ERROR_INVALID_PARAMS, *message)
            }
            error => {
                eprintln!("zeroshot acp: {error}");
                acp::Error::new(ACP_ERROR_INTERNAL, "Zeroshot ACP operation failed")
            }
        }
    }
}

/// Serves one local profile over ACP stdio until the client disconnects.
pub async fn serve_local_acp(profile_name: RunProfileName) -> Result<(), AcpServeError> {
    let mut profile = LocalRunProfileStore::production()?.show(RunProfileSelector {
        scope: RunProfileScope::User,
        name: profile_name,
    })?;
    materialize_acp_provider_access(&mut profile.runtime)?;
    validate_profile(&profile).await?;
    let core = Arc::new(AcpCore::new(profile, default_local_state_root()?));
    let local = tokio::task::LocalSet::new();
    local
        .run_until(serve_stdio(core))
        .await
        .map_err(|error| AcpServeError::Transport(error.to_string()))
}

fn materialize_acp_provider_access(runtime: &mut RuntimePlan) -> Result<(), AcpServeError> {
    materialize_provider_access(runtime, ProviderAccessPlacement::Local)
        .map_err(|error| NativeV2CliError::Usage(error.to_string()).into())
}

async fn serve_stdio(core: Arc<AcpCore>) -> acp::Result<()> {
    let (updates, mut update_receiver) = mpsc::unbounded_channel();
    let handler = AcpAgent {
        core: core.clone(),
        updates,
    };
    let (connection, io) = acp::AgentSideConnection::new(
        handler,
        tokio::io::stdout().compat_write(),
        tokio::io::stdin().compat(),
        |future| {
            tokio::task::spawn_local(future);
        },
    );
    let publisher = tokio::task::spawn_local(async move {
        while let Some((notification, acknowledgement)) = update_receiver.recv().await {
            let sent = connection.session_notification(notification).await.is_ok();
            let _ = acknowledgement.send(sent.then_some(()).ok_or(()));
            if !sent {
                break;
            }
        }
    });
    let result = io.await;
    core.close_all().await;
    publisher.abort();
    result
}

struct AcpAgent {
    core: Arc<AcpCore>,
    updates: mpsc::UnboundedSender<SessionUpdate>,
}

#[async_trait(?Send)]
impl acp::Agent for AcpAgent {
    async fn initialize(
        &self,
        _request: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        let capabilities = acp::AgentCapabilities::new().session_capabilities(
            acp::SessionCapabilities::new().close(acp::SessionCloseCapabilities::new()),
        );
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_capabilities(capabilities)
            .agent_info(
                acp::Implementation::new("zeroshot", env!("CARGO_PKG_VERSION")).title("Zeroshot"),
            ))
    }

    async fn authenticate(
        &self,
        _request: acp::AuthenticateRequest,
    ) -> Result<acp::AuthenticateResponse, acp::Error> {
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        request: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, acp::Error> {
        self.core
            .new_session(request)
            .await
            .map(acp::NewSessionResponse::new)
            .map_err(|error| error.rpc_error())
    }

    async fn prompt(&self, request: acp::PromptRequest) -> Result<acp::PromptResponse, acp::Error> {
        let turn = self
            .core
            .prompt(&request.session_id, task_from_prompt(request.prompt)?)
            .await
            .map_err(|error| error.rpc_error())?;
        let response = acp::PromptResponse::new(turn.stop_reason).meta(turn.meta());
        if let Some(message) = turn.message {
            publish_message(
                &self.updates,
                request.session_id.clone(),
                message,
                turn.run_id.clone(),
            )
            .await?;
        }
        Ok(response)
    }

    async fn cancel(&self, request: acp::CancelNotification) -> Result<(), acp::Error> {
        self.core
            .cancel(&request.session_id)
            .await
            .map_err(|error| error.rpc_error())
    }

    async fn close_session(
        &self,
        request: acp::CloseSessionRequest,
    ) -> Result<acp::CloseSessionResponse, acp::Error> {
        self.core
            .close(&request.session_id)
            .await
            .map(|()| acp::CloseSessionResponse::default())
            .map_err(|error| error.rpc_error())
    }
}

async fn publish_message(
    updates: &mpsc::UnboundedSender<SessionUpdate>,
    session_id: acp::SessionId,
    message: String,
    run_id: RunId,
) -> Result<(), acp::Error> {
    let notification = acp::SessionNotification::new(
        session_id,
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(message.into())),
    )
    .meta(acp_meta(run_id, None));
    let (acknowledgement, received) = oneshot::channel();
    updates
        .send((notification, acknowledgement))
        .map_err(|_| acp::Error::internal_error())?;
    received
        .await
        .map_err(|_| acp::Error::internal_error())?
        .map_err(|()| acp::Error::internal_error())
}

fn task_from_prompt(prompt: Vec<acp::ContentBlock>) -> Result<String, acp::Error> {
    let [acp::ContentBlock::Text(text)] = prompt.as_slice() else {
        return Err(acp::Error::new(
            ACP_ERROR_INVALID_PARAMS,
            "Zeroshot ACP accepts exactly one text block as task",
        ));
    };
    if text.text.is_empty() {
        return Err(acp::Error::new(
            ACP_ERROR_INVALID_PARAMS,
            "task must not be empty",
        ));
    }
    Ok(text.text.clone())
}

struct AcpCore {
    profile: RunProfile,
    state_root: PathBuf,
    session: Mutex<Option<Arc<AcpSession>>>,
}

impl AcpCore {
    fn new(profile: RunProfile, state_root: PathBuf) -> Self {
        Self {
            profile,
            state_root,
            session: Mutex::new(None),
        }
    }

    async fn new_session(
        &self,
        request: acp::NewSessionRequest,
    ) -> Result<acp::SessionId, AcpServeError> {
        if !request.mcp_servers.is_empty() || !request.additional_directories.is_empty() {
            return Err(AcpServeError::Request(
                "MCP servers and additional directories are not supported",
            ));
        }
        let mut slot = self.session.lock().await;
        if slot.is_some() {
            return Err(AcpServeError::Request(
                "this ACP process already has an active session",
            ));
        }
        let session =
            AcpSession::create(self.profile.clone(), self.state_root.clone(), request.cwd).await?;
        let session_id = session.id.clone();
        *slot = Some(session);
        Ok(session_id)
    }

    async fn prompt(
        &self,
        session_id: &acp::SessionId,
        task: String,
    ) -> Result<TurnResult, AcpServeError> {
        self.require_session(session_id).await?.run_turn(task).await
    }

    async fn cancel(&self, session_id: &acp::SessionId) -> Result<(), AcpServeError> {
        self.require_session(session_id).await?.cancel().await;
        Ok(())
    }

    async fn close(&self, session_id: &acp::SessionId) -> Result<(), AcpServeError> {
        let mut slot = self.session.lock().await;
        let session = slot
            .as_ref()
            .filter(|session| &session.id == session_id)
            .cloned()
            .ok_or(AcpServeError::Request("session was not found"))?;
        session.close().await;
        *slot = None;
        Ok(())
    }

    async fn close_all(&self) {
        let mut slot = self.session.lock().await;
        if let Some(session) = slot.as_ref().cloned() {
            session.close().await;
            *slot = None;
        }
    }

    async fn require_session(
        &self,
        session_id: &acp::SessionId,
    ) -> Result<Arc<AcpSession>, AcpServeError> {
        self.session
            .lock()
            .await
            .as_ref()
            .filter(|session| &session.id == session_id)
            .cloned()
            .ok_or(AcpServeError::Request("session was not found"))
    }
}

struct AcpSession {
    id: acp::SessionId,
    profile: RunProfile,
    environment: Arc<RunEnvironment>,
    workspace: PathBuf,
    runner: Arc<NativeNodeRunner>,
    resources: SyncMutex<Option<SessionResources>>,
    state: SyncMutex<SessionState>,
    turn: Mutex<()>,
    state_root: PathBuf,
}

struct SessionResources {
    identity: WorkspaceIdentity,
    workspace_lease: Arc<ControllerLease>,
    _runtime: tempfile::TempDir,
}

#[derive(Default)]
struct SessionState {
    active: Option<Arc<ActiveTurn>>,
    closed: bool,
    lost: bool,
}

impl SessionState {
    fn start_turn(&mut self) -> Result<Arc<ActiveTurn>, AcpServeError> {
        if self.closed {
            return Err(AcpServeError::Request("session is closed"));
        }
        if self.lost {
            return Err(AcpServeError::Request(
                "session workspace ownership was lost",
            ));
        }
        if self.active.is_some() {
            return Err(AcpServeError::Request("another prompt is already active"));
        }
        let active = Arc::new(ActiveTurn::new());
        self.active = Some(active.clone());
        Ok(active)
    }

    fn cancel_target(&mut self) -> Option<Arc<ActiveTurn>> {
        if self.closed || self.lost {
            return None;
        }
        self.active.clone()
    }

    fn close_target(&mut self) -> Result<Option<Arc<ActiveTurn>>, ()> {
        if self.closed {
            return Err(());
        }
        self.closed = true;
        Ok(self.active.clone())
    }

    fn loss_target(&mut self) -> Option<Arc<ActiveTurn>> {
        if self.closed || self.lost {
            return None;
        }
        self.lost = true;
        self.active.clone()
    }

    fn finish_turn(&mut self, active: &Arc<ActiveTurn>) {
        if self
            .active
            .as_ref()
            .is_some_and(|candidate| Arc::ptr_eq(candidate, active))
        {
            self.active = None;
        }
    }
}

struct PreparedSession {
    workspace: PathBuf,
    source: openengine_cluster_protocol::ResolvedSource,
    resources: SessionResources,
}

struct PreparedTurn {
    run_id: RunId,
    ledger: Arc<SqliteRunLedger>,
    leases: Arc<TurnLeases>,
}

struct TurnLeases {
    // Fields drop in declaration order. Release the ACP identity before controller ownership.
    acp_turn: ControllerLease,
    controller: ControllerLease,
}

impl TurnLeases {
    fn acquire(paths: &PortableControllerPaths) -> Result<Self, ControllerLeaseError> {
        let controller = ControllerLease::acquire(paths.lease())?;
        let acp_turn = ControllerLease::acquire(paths.acp_turn_lease())?;
        Ok(Self {
            acp_turn,
            controller,
        })
    }

    fn is_intact(&self) -> bool {
        self.controller.is_intact() && self.acp_turn.is_intact()
    }
}

struct ActiveTurn {
    cancelled: AtomicBool,
    lost: AtomicBool,
    leases: SyncMutex<Option<Arc<TurnLeases>>>,
    supervisor: Mutex<Option<Arc<NativeV2Supervisor>>>,
}

impl ActiveTurn {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            lost: AtomicBool::new(false),
            leases: SyncMutex::new(None),
            supervisor: Mutex::new(None),
        }
    }

    fn set_leases(&self, leases: Arc<TurnLeases>) {
        *self.leases.lock().expect("active turn lease lock") = Some(leases);
    }

    fn leases_are_intact(&self) -> bool {
        self.leases
            .lock()
            .expect("active turn lease lock")
            .as_ref()
            .is_none_or(|leases| leases.is_intact())
    }

    async fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(supervisor) = self.supervisor.lock().await.clone() {
            let _ = supervisor.force_stop().await;
        }
    }

    async fn runtime_lost(&self) {
        self.lost.store(true, Ordering::Release);
        if let Some(supervisor) = self.supervisor.lock().await.clone() {
            supervisor.runtime_lost().await;
        }
    }
}

impl AcpSession {
    async fn create(
        profile: RunProfile,
        state_root: PathBuf,
        requested_workspace: PathBuf,
    ) -> Result<Arc<Self>, AcpServeError> {
        let prepared = prepare_session(&state_root, requested_workspace)?;
        let environment = Arc::new(RunEnvironment::exact(
            &profile.runtime,
            RunConnectionValues::new(),
        )?);
        let seed_run_id = new_run_id();
        let admitted = NativeV2Admission
            .admit(submission(
                &profile,
                &prepared.source,
                "",
                seed_run_id.as_str(),
            )?)
            .await?;
        let invoking_directory = std::env::current_dir().map_err(AcpServeError::Storage)?;
        let native_environment =
            crate::native_v2_local::capture_local_native_environment(&invoking_directory)?;
        let runner = build_local_owner_process_candidate(LocalProcessCandidateRequest {
            admitted: &admitted,
            delivery_run_id: seed_run_id,
            adopt_existing_delivery: false,
            workspace: &prepared.workspace,
            storage: prepared.resources._runtime.path(),
            github_token: None,
            native_environment: &native_environment,
        })?;
        let session = Arc::new(Self {
            id: acp::SessionId::new(uuid::Uuid::now_v7().to_string()),
            profile,
            environment,
            workspace: prepared.workspace,
            runner,
            resources: SyncMutex::new(Some(prepared.resources)),
            state: SyncMutex::new(SessionState::default()),
            turn: Mutex::new(()),
            state_root,
        });
        monitor_session(Arc::downgrade(&session));
        Ok(session)
    }

    async fn run_turn(&self, task: String) -> Result<TurnResult, AcpServeError> {
        let _turn = self
            .turn
            .try_lock()
            .map_err(|_| AcpServeError::Request("another prompt is already active"))?;
        let active = {
            let mut state = self.state.lock().expect("ACP session state lock");
            state.start_turn()?
        };
        let result = if self.workspace_is_intact() {
            self.run_turn_inner(task, active.clone()).await
        } else {
            self.lose_workspace().await;
            Err(AcpServeError::Request(
                "session workspace ownership was lost",
            ))
        };
        let mut state = self.state.lock().expect("ACP session state lock");
        state.finish_turn(&active);
        result
    }

    async fn run_turn_inner(
        &self,
        task: String,
        active: Arc<ActiveTurn>,
    ) -> Result<TurnResult, AcpServeError> {
        let prepared = self.prepare_turn(task).await?;
        active.set_leases(prepared.leases.clone());
        let runner: Arc<dyn crate::native_v2_runner::NodeRunner> = self.runner.clone();
        let supervisor = Arc::new(NativeV2Supervisor::new(
            prepared.run_id.clone(),
            prepared.ledger,
            runner,
            self.environment.clone(),
        ));
        *active.supervisor.lock().await = Some(supervisor.clone());
        if !self.workspace_is_intact() || !active.leases_are_intact() {
            self.lose_workspace().await;
        }
        if active.lost.load(Ordering::Acquire) {
            supervisor.runtime_lost().await;
        } else if active.cancelled.load(Ordering::Acquire) {
            // Drive observes in-memory stop intent and owns persistence failure recovery.
            let _ = supervisor.force_stop().await;
        }
        let terminal = drive_supervisor(&supervisor).await?;
        let cancelled = active.cancelled.load(Ordering::Acquire);
        TurnResult::new(prepared.run_id, terminal, cancelled)
    }

    async fn prepare_turn(&self, task: String) -> Result<PreparedTurn, AcpServeError> {
        let run_id = new_run_id();
        let (workspace, source) = local_resolved_source(&self.workspace, Path::new("git"))?;
        if workspace != self.workspace {
            return Err(AcpServeError::Portable(PortableControllerError::Workspace));
        }
        let submission = submission(&self.profile, &source, &task, run_id.as_str())?;
        let digest = submission_digest(&submission)?;
        let submission_key = submission.submission_key.clone();
        let admitted = NativeV2Admission.admit(submission).await?;
        let storage = create_local_run_storage(&self.state_root, &run_id)?;
        let paths = PortableControllerPaths::new(storage);
        let leases = Arc::new(TurnLeases::acquire(&paths)?);
        let ledger = Arc::new(SqliteRunLedger::open(paths.ledger())?);
        ledger
            .create_or_get(CreateRun {
                run_id: run_id.clone(),
                submission_key,
                submission_digest: digest,
                admitted,
            })
            .await?;
        Ok(PreparedTurn {
            run_id,
            ledger,
            leases,
        })
    }

    async fn cancel(&self) {
        let active = {
            let mut state = self.state.lock().expect("ACP session state lock");
            state.cancel_target()
        };
        if let Some(active) = active {
            active.cancel().await;
        }
    }

    async fn close(&self) {
        let active = {
            let mut state = self.state.lock().expect("ACP session state lock");
            let Ok(active) = state.close_target() else {
                return;
            };
            active
        };
        if let Some(active) = active {
            active.cancel().await;
        }
        let _turn = self.turn.lock().await;
        self.runner.close_session().await;
        self.resources
            .lock()
            .expect("ACP session resources lock")
            .take();
    }

    fn workspace_is_intact(&self) -> bool {
        let resources = self.resources.lock().expect("ACP session resources lock");
        resources.as_ref().is_some_and(|resources| {
            resources.identity.is_current(&self.workspace) && resources.workspace_lease.is_intact()
        })
    }

    fn monitored_turn(&self) -> Option<Arc<ActiveTurn>> {
        let state = self.state.lock().expect("ACP session state lock");
        (!state.closed).then(|| state.active.clone()).flatten()
    }

    async fn lose_workspace(&self) {
        let active = {
            let mut state = self.state.lock().expect("ACP session state lock");
            state.loss_target()
        };
        if let Some(active) = active {
            active.runtime_lost().await;
        }
    }
}

fn monitor_session(session: Weak<AcpSession>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(WORKSPACE_MONITOR_INTERVAL).await;
            let Some(session) = session.upgrade() else {
                return;
            };
            let active = session.monitored_turn();
            let intact = session.workspace_is_intact()
                && active.as_ref().is_none_or(|turn| turn.leases_are_intact());
            if !intact {
                session.lose_workspace().await;
                return;
            }
            if session.state.lock().expect("ACP session state lock").closed {
                return;
            }
        }
    });
}

fn prepare_session(
    state_root: &Path,
    requested_workspace: PathBuf,
) -> Result<PreparedSession, AcpServeError> {
    if !requested_workspace.is_absolute() {
        return Err(AcpServeError::Request("session cwd must be absolute"));
    }
    let requested_workspace =
        std::fs::canonicalize(&requested_workspace).map_err(AcpServeError::Storage)?;
    let (workspace, source) = local_resolved_source(&requested_workspace, Path::new("git"))?;
    if requested_workspace != workspace {
        return Err(AcpServeError::Request(
            "session cwd must be the canonical Git workspace root",
        ));
    }
    let resources = prepare_session_resources(state_root, &workspace)?;
    Ok(PreparedSession {
        workspace,
        source,
        resources,
    })
}

fn prepare_session_resources(
    state_root: &Path,
    workspace: &Path,
) -> Result<SessionResources, AcpServeError> {
    let identity = WorkspaceIdentity::capture(workspace)?;
    let workspace_lease = Arc::new(ControllerLease::acquire(local_workspace_lease_path(
        state_root, workspace,
    )?)?);
    if !identity.is_current(workspace) {
        return Err(AcpServeError::Portable(PortableControllerError::Workspace));
    }
    let acp_root = state_root.join("acp");
    crate::execution::platform::private_directory(&acp_root).map_err(AcpServeError::Storage)?;
    let runtime = tempfile::Builder::new()
        .prefix("session-")
        .tempdir_in(acp_root)
        .map_err(AcpServeError::Storage)?;
    Ok(SessionResources {
        identity,
        workspace_lease,
        _runtime: runtime,
    })
}

async fn drive_supervisor(
    supervisor: &NativeV2Supervisor,
) -> Result<TerminalResult, AcpServeError> {
    match supervisor.drive().await {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            let _ = supervisor.fail_runtime(|| {}).await;
            Err(error.into())
        }
    }
}

struct TurnResult {
    run_id: RunId,
    raw_output: Value,
    message: Option<String>,
    stop_reason: acp::StopReason,
}

impl TurnResult {
    fn new(
        run_id: RunId,
        terminal: TerminalResult,
        cancelled: bool,
    ) -> Result<Self, AcpServeError> {
        if cancelled {
            return Ok(Self {
                run_id,
                raw_output: terminal_value(terminal),
                message: None,
                stop_reason: acp::StopReason::Cancelled,
            });
        }
        match terminal {
            TerminalResult::Succeeded { output } => Ok(Self {
                run_id,
                message: Some(response_string(&output)?.to_owned()),
                raw_output: output,
                stop_reason: acp::StopReason::EndTurn,
            }),
            TerminalResult::Failed { reason } => Ok(Self {
                run_id,
                raw_output: json!({ "failed": reason.as_str() }),
                message: Some(format!("Zeroshot run failed: {}", reason.as_str())),
                stop_reason: acp::StopReason::EndTurn,
            }),
        }
    }

    fn meta(&self) -> acp::Meta {
        acp_meta(self.run_id.clone(), Some(self.raw_output.clone()))
    }
}

fn terminal_value(terminal: TerminalResult) -> Value {
    match terminal {
        TerminalResult::Succeeded { output } => output,
        TerminalResult::Failed { reason } => json!({ "failed": reason.as_str() }),
    }
}

fn response_string(output: &Value) -> Result<&str, AcpServeError> {
    output
        .as_object()
        .and_then(|output| output.get("response"))
        .and_then(Value::as_str)
        .ok_or(AcpServeError::Profile(
            "successful output must contain exactly one response string",
        ))
}

fn acp_meta(run_id: RunId, raw_output: Option<Value>) -> acp::Meta {
    let mut zeroshot = Map::from_iter([("runId".to_owned(), json!(run_id.as_str()))]);
    if let Some(raw_output) = raw_output {
        zeroshot.insert("rawOutput".to_owned(), raw_output);
    }
    Map::from_iter([("zeroshot".to_owned(), Value::Object(zeroshot))])
}

fn new_run_id() -> RunId {
    RunId::new(uuid::Uuid::now_v7().to_string())
}

fn submission(
    profile: &RunProfile,
    source: &openengine_cluster_protocol::ResolvedSource,
    task: &str,
    identity: &str,
) -> Result<RunSubmission, AcpServeError> {
    Ok(RunSubmission {
        title: RunTitle::new("ACP turn").map_err(|_| AcpServeError::Profile("invalid title"))?,
        graph: profile.graph.clone(),
        initial_input: json!({ "task": task }),
        runtime: profile.runtime.clone(),
        source: source.clone(),
        submission_key: IdempotencyKey::new(format!("acp-{identity}"))
            .map_err(|_| AcpServeError::Profile("invalid submission identity"))?,
    })
}

async fn validate_profile(profile: &RunProfile) -> Result<(), AcpServeError> {
    validate_task_type(&profile.graph.initial_input)?;
    if !matches!(
        profile.runtime,
        RuntimePlan::Codex { .. } | RuntimePlan::Claude { .. }
    ) {
        return Err(AcpServeError::Profile(
            "only Codex and Claude runtimes are supported",
        ));
    }
    let mut success_count = 0;
    validate_node(&profile.graph.root, &mut success_count)?;
    if success_count == 0 {
        return Err(AcpServeError::Profile(
            "graph must contain at least one success node",
        ));
    }
    if !profile.runtime.connection_requirements().is_empty() {
        return Err(AcpServeError::Profile(
            "runtime connections are not supported by the ACP MVP",
        ));
    }
    if profile.runtime.nodes().values().any(|binding| {
        !matches!(
            binding,
            NodeRuntimeBinding::Agent {
                session_scope: SessionScope::NodeInstance,
                ..
            }
        )
    }) {
        return Err(AcpServeError::Profile(
            "every node must be an agent with node_instance session scope",
        ));
    }
    NativeV2Admission
        .validate_profile(&profile.graph, &profile.runtime, DeliveryPolicy::Optional)
        .await?;
    Ok(())
}

fn validate_node(node: &GraphNode, success_count: &mut usize) -> Result<(), AcpServeError> {
    match node {
        GraphNode::Map(_) => {
            return Err(AcpServeError::Profile("map nodes are not supported"));
        }
        GraphNode::Succeed(success) => {
            validate_response_type(&success.output)?;
            *success_count += 1;
        }
        _ => {}
    }
    for child in openengine_cluster_server::graph_verifier::graph_node_children(node) {
        validate_node(child, success_count)?;
    }
    Ok(())
}

fn validate_task_type(payload: &PayloadType) -> Result<(), AcpServeError> {
    validate_single_string_record(payload, "task").map_err(|()| {
        AcpServeError::Profile("initial input must be exactly one required task string")
    })
}

fn validate_response_type(payload: &PayloadType) -> Result<(), AcpServeError> {
    validate_single_string_record(payload, "response").map_err(|()| {
        AcpServeError::Profile("every success output must be exactly one required response string")
    })
}

fn validate_single_string_record(payload: &PayloadType, field: &str) -> Result<(), ()> {
    let PayloadType::Record { fields } = payload else {
        return Err(());
    };
    let name = FieldName::new(field).map_err(|_| ())?;
    let Some(value) = fields.get(&name) else {
        return Err(());
    };
    (fields.len() == 1 && value.required && value.value_type == PayloadType::String)
        .then_some(())
        .ok_or(())
}

#[cfg(test)]
#[path = "acp/tests.rs"]
mod tests;
