use std::io;
use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    GetParams, GetResult, InitializeParams, InitializeResult, RunAttachParams, RunAttachResult,
    RunCheckpointsParams, RunCheckpointsResult, RunDiscardWorkspaceParams,
    RunDiscardWorkspaceResult, RunForceParams, RunForceResult, RunListParams, RunListResult,
    RunLogsParams, RunLogsResult, RunResumeParams, RunResumeResult, RunStatusParams,
    RunStatusResult, RunSubmitParams, RunSubmitResult, RunWatchParams, RunWatchResult,
    TargetOecpSessionRequest, TargetPrivateBootstrapRequest, INVALID_PHASE, RUN_CONFLICT,
    SCHEMA_VIOLATION, TARGET_PRIVATE_BOOTSTRAP_PATH, is_canonical_uuid_v7,
};
use openengine_cluster_server::admission::CancellationSignal;
use openengine_cluster_server::identity::{
    ConnectionBinding, ConnectionIdentity, StaticConnectionIdentityResolver, SystemConnectionTime,
};
use openengine_cluster_server::native_v2::{
    RunAttachEventStream, RunLogEventStream, RunWatchEventStream,
};
use openengine_cluster_server::{BackendError, ClusterBackend, ConnectionContext};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio_tungstenite::accept_async_with_config;
use url::Url;

use super::{
    DISCOVERY_PATH, NativeV2TargetAuthority, OECP_PATH, OPERATOR_DIAGNOSTICS_PATH_PREFIX, RUN_PATH,
    SESSION_PATH, TargetAuthentication, TargetAuthorityError, TargetDiscoveryDocument,
    TargetOecpSession, TargetRunRequest, TargetSessionAuthority,
};
use crate::native_v2_cloud::NativeV2CloudController;
use super::private_access::{PrivateTargetAccess, TargetBootstrapKey};

#[path = "transport_history.rs"]
mod history;
#[path = "transport_http.rs"]
mod http;
use http::{
    HttpRequest, HttpResponse, RequestHead, authority_error_response, peek_request_head,
    invalid_request_response, not_found_response, read_http_request, request_timeout_response,
    run_error_response, unavailable_response, write_and_close, write_http_response,
};

const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_PRIVATE_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_BEARER_BYTES: usize = 16 * 1024;
const REQUEST_HEAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const CONNECTION_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 256;

/// Concrete target-wide HTTP/WebSocket binding. Hosted access consumes bearer authority supplied
/// by its host. Explicit direct access uses one static target identity and no Authorization.
pub struct NativeV2TargetServer {
    target: Arc<NativeV2TargetAuthority>,
    access: TargetServerAccess,
    oecp_endpoint: String,
    #[cfg(feature = "ui")]
    ui: Option<crate::profile_ui::UiService>,
}

enum TargetServerAccess {
    Hosted(Arc<dyn TargetSessionAuthority>),
    Private {
        access: Arc<PrivateTargetAccess>,
        identity: ConnectionIdentity,
    },
    Direct(ConnectionIdentity),
}

impl TargetServerAccess {
    const fn authentication(&self) -> TargetAuthentication {
        match self {
            Self::Hosted(_) => TargetAuthentication::HostedOauth,
            Self::Private { .. } => TargetAuthentication::PrivateCapability,
            Self::Direct(_) => TargetAuthentication::None,
        }
    }

    const fn permits_workspace_recovery(&self) -> bool {
        !matches!(self, Self::Hosted(_))
    }
}

impl NativeV2TargetServer {
    pub fn new_hosted(
        target: Arc<NativeV2TargetAuthority>,
        sessions: Arc<dyn TargetSessionAuthority>,
        oecp_endpoint: impl Into<String>,
    ) -> Result<Self, TargetAuthorityError> {
        let endpoint = oecp_endpoint.into();
        validate_oecp_endpoint(&endpoint)?;
        Ok(Self {
            target,
            access: TargetServerAccess::Hosted(sessions),
            oecp_endpoint: endpoint,
            #[cfg(feature = "ui")]
            ui: None,
        })
    }

    pub fn new_direct(
        target: Arc<NativeV2TargetAuthority>,
        identity: ConnectionIdentity,
        oecp_endpoint: impl Into<String>,
    ) -> Result<Self, TargetAuthorityError> {
        let endpoint = oecp_endpoint.into();
        validate_oecp_endpoint(&endpoint)?;
        Ok(Self {
            target,
            access: TargetServerAccess::Direct(identity),
            oecp_endpoint: endpoint,
            #[cfg(feature = "ui")]
            ui: None,
        })
    }

    pub fn new_private(
        target: Arc<NativeV2TargetAuthority>,
        identity: ConnectionIdentity,
        oecp_endpoint: impl Into<String>,
        bootstrap_key: TargetBootstrapKey,
    ) -> Result<Self, TargetAuthorityError> {
        let endpoint = oecp_endpoint.into();
        validate_oecp_endpoint(&endpoint)?;
        Ok(Self {
            target,
            access: TargetServerAccess::Private {
                access: Arc::new(PrivateTargetAccess::new(bootstrap_key)),
                identity,
            },
            oecp_endpoint: endpoint,
            #[cfg(feature = "ui")]
            ui: None,
        })
    }

    /// Mounts the standalone workspace only on an explicitly unauthenticated direct target.
    /// Private and hosted targets need their host's authenticated UI adapter instead.
    #[cfg(feature = "ui")]
    pub fn with_ui(
        mut self,
        ui: crate::profile_ui::UiService,
    ) -> Result<Self, TargetAuthorityError> {
        if !matches!(self.access, TargetServerAccess::Direct(_)) {
            return Err(TargetAuthorityError::invalid(
                "the standalone UI requires direct target access",
            ));
        }
        self.ui = Some(ui);
        Ok(self)
    }

    /// Serves a supplied listener. Cloud hosting may instead call [`Self::serve_connection`] from
    /// its existing listener/TLS lifecycle.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) -> io::Result<()> {
        self.serve_until(listener, std::future::pending()).await
    }

    /// Stops accepting on host shutdown, closes UI subscriptions, and bounds connection draining.
    /// These connections only observe/control the separately owned target; closing them never
    /// synthesizes a run cancellation. Dropping this future also drops its owned connection tasks.
    pub async fn serve_until(
        self: Arc<Self>,
        listener: TcpListener,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        tokio::pin!(shutdown);
        let mut connections = JoinSet::new();
        let result = loop {
            tokio::select! {
                biased;
                () = &mut shutdown => break Ok(()),
                accepted = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                    let (stream, _) = match accepted {
                        Ok(accepted) => accepted,
                        Err(error) => break Err(error),
                    };
                    let server = self.clone();
                    connections.spawn(async move { server.serve_connection(stream).await });
                }
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        };
        drop(listener);
        #[cfg(feature = "ui")]
        if let Some(ui) = &self.ui {
            ui.shutdown();
        }
        let drained = tokio::time::timeout(CONNECTION_DRAIN_TIMEOUT, async {
            while connections.join_next().await.is_some() {}
        })
        .await;
        if drained.is_err() {
            connections.shutdown().await;
        }
        result
    }

    /// Routes one real TCP connection. WebSocket handshakes remain on the same target authority
    /// as discovery/session, and the resulting OECP backend is the shared target controller.
    pub async fn serve_connection(&self, mut stream: TcpStream) -> io::Result<()> {
        let head = match read_request_head(&stream).await {
            Ok(head) => head,
            Err(error) => return write_request_error(&mut stream, error).await,
        };
        #[cfg(feature = "ui")]
        if head.is_ui_route() {
            if let Some(ui) = &self.ui {
                // The UI router owns every subsequent request on this connection. Its route
                // set excludes native control endpoints, including HTTP keep-alive requests.
                return ui.serve_connection(stream).await;
            }
        }
        if head.is_websocket_upgrade() {
            return self.serve_oecp(stream, head).await;
        }
        let request = match read_http_request(&mut stream, head).await {
            Ok(request) => request,
            Err(error) => return write_request_error(&mut stream, error).await,
        };
        let response = self.handle_http(request).await;
        write_http_response(&mut stream, response).await
    }

    async fn serve_oecp(&self, stream: TcpStream, head: RequestHead) -> io::Result<()> {
        if head.method != "GET" || head.path != OECP_PATH {
            return write_and_close(stream, not_found_response()).await;
        }
        let identity = match self.authenticate_oecp(&head).await {
            Ok(identity) => identity,
            Err(error) => return write_and_close(stream, authority_error_response(error)).await,
        };
        let controller = match self.target.controller().await {
            Ok(controller) => controller,
            Err(error) => {
                return write_and_close(stream, authority_error_response(error)).await;
            }
        };
        let websocket = accept_async_with_config(
            stream,
            Some(openengine_cluster_server::websocket::websocket_config()),
        )
        .await
        .map_err(io::Error::other)?;
        let binding = ConnectionBinding::new(
            Arc::new(TargetOecpBackend {
                controller,
                workspace_recovery: self.workspace_recovery(),
                workspace_checkpoints: self.workspace_checkpoints(),
            }),
            StaticConnectionIdentityResolver::new(identity),
            SystemConnectionTime,
            CancellationSignal::default(),
        );
        openengine_cluster_server::websocket::serve_websocket(binding, websocket).await
    }

    async fn handle_http(&self, request: HttpRequest) -> HttpResponse {
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", DISCOVERY_PATH) if request.body.is_empty() => {
                HttpResponse::json(200, &self.discovery_document())
            }
            ("POST", TARGET_PRIVATE_BOOTSTRAP_PATH) => self.handle_private_bootstrap(request).await,
            ("GET", path) if is_operator_diagnostics_path(path) => {
                self.handle_operator_diagnostics(request).await
            }
            ("POST", history::DEFINITION_PATH | history::PAGE_PATH) => {
                self.handle_history(request).await
            }
            ("POST", RUN_PATH) => self.handle_run(request).await,
            ("POST", SESSION_PATH) => self.handle_session(request).await,
            _ => not_found_response(),
        }
    }

    async fn handle_operator_diagnostics(&self, request: HttpRequest) -> HttpResponse {
        if let Err(response) = self.authenticate_private_control(&request.head).await {
            return response;
        }
        if !request.body.is_empty() {
            return invalid_request_response("operator diagnostic request is malformed");
        }
        let Some(run_id) = operator_diagnostics_run_id(&request.path) else {
            return invalid_request_response("operator diagnostic request is malformed");
        };
        HttpResponse::private_json(200, &self.target.operator_diagnostics(&run_id))
    }

    async fn handle_private_bootstrap(&self, request: HttpRequest) -> HttpResponse {
        let TargetServerAccess::Private { access, .. } = &self.access else {
            return not_found_response();
        };
        let request = match serde_json::from_slice::<TargetPrivateBootstrapRequest>(&request.body) {
            Ok(request) => request,
            Err(_) => return invalid_request_response("private bootstrap request is malformed"),
        };
        match access.bootstrap(&request).await {
            Ok(()) => HttpResponse::empty(204),
            Err(error) if error.kind() == super::TargetAuthorityErrorKind::Unavailable => {
                not_found_response()
            }
            Err(error) => authority_error_response(error),
        }
    }

    async fn handle_run(&self, request: HttpRequest) -> HttpResponse {
        if let Err(error) = self.authenticate_control(&request.head).await {
            return authority_error_response(error);
        }
        let submission = match serde_json::from_slice::<TargetRunRequest>(&request.body) {
            Ok(submission) => submission,
            Err(_) => return invalid_request_response("target run request is malformed"),
        };
        if !is_canonical_uuid_v7(&submission.run_id) {
            return invalid_request_response("target run ID must be a canonical UUIDv7");
        }
        match self.target.submit(submission).await {
            Ok(receipt) => HttpResponse::private_json(200, &receipt),
            Err(error) => run_error_response(error),
        }
    }

    async fn handle_session(&self, request: HttpRequest) -> HttpResponse {
        let identity = match self.authenticate_control(&request.head).await {
            Ok(identity) => identity,
            Err(error) => return authority_error_response(error),
        };
        let session_request =
            match serde_json::from_slice::<TargetOecpSessionRequest>(&request.body) {
                Ok(request) if request.run_id.as_ref().is_none_or(is_canonical_uuid_v7) => request,
                _ => return invalid_request_response("target OECP session request is malformed"),
            };
        if let Err(error) = self.target.controller().await {
            return authority_error_response(error);
        }
        self.issue_session(&identity, &session_request).await
    }

    async fn issue_session(
        &self,
        identity: &openengine_cluster_server::identity::ConnectionIdentity,
        request: &TargetOecpSessionRequest,
    ) -> HttpResponse {
        match &self.access {
            TargetServerAccess::Hosted(sessions) => {
                match sessions.issue_oecp(identity, request).await {
                    Ok(bearer_token) if valid_issued_bearer(&bearer_token) => {
                        HttpResponse::private_json(
                            200,
                            &TargetOecpSession {
                                endpoint: self.oecp_endpoint.clone(),
                                bearer_token: Some(bearer_token),
                            },
                        )
                    }
                    Ok(_) => unavailable_response(),
                    Err(error) => authority_error_response(error),
                }
            }
            TargetServerAccess::Private { access, .. } => match access.token().await {
                Ok(bearer_token) => HttpResponse::private_json(
                    200,
                    &TargetOecpSession {
                        endpoint: self.oecp_endpoint.clone(),
                        bearer_token: Some(bearer_token),
                    },
                ),
                Err(error) => authority_error_response(error),
            },
            TargetServerAccess::Direct(_) => HttpResponse::private_json(
                200,
                &TargetOecpSession {
                    endpoint: self.oecp_endpoint.clone(),
                    bearer_token: None,
                },
            ),
        }
    }

    async fn authenticate_private_control(&self, head: &RequestHead) -> Result<(), HttpResponse> {
        if !matches!(self.access, TargetServerAccess::Private { .. }) {
            return Err(not_found_response());
        }
        self.authenticate_control(head)
            .await
            .map(|_| ())
            .map_err(authority_error_response)
    }

    async fn authenticate_control(
        &self,
        head: &RequestHead,
    ) -> Result<openengine_cluster_server::identity::ConnectionIdentity, TargetAuthorityError> {
        self.authenticate(head, BearerPurpose::Control).await
    }

    fn workspace_recovery(&self) -> bool {
        self.access.permits_workspace_recovery() && self.target.supports_workspace_recovery()
    }

    fn workspace_checkpoints(&self) -> bool {
        self.workspace_recovery() && self.target.supports_workspace_checkpoints()
    }

    fn discovery_document(&self) -> TargetDiscoveryDocument {
        let document = TargetDiscoveryDocument::direct(self.access.authentication());
        let document = if self.workspace_recovery() {
            document.with_workspace_recovery()
        } else {
            document
        };
        if self.workspace_checkpoints() {
            document.with_workspace_checkpoints()
        } else {
            document
        }
    }

    async fn authenticate_oecp(
        &self,
        head: &RequestHead,
    ) -> Result<ConnectionIdentity, TargetAuthorityError> {
        self.authenticate(head, BearerPurpose::Oecp).await
    }

    async fn authenticate(
        &self,
        head: &RequestHead,
        purpose: BearerPurpose,
    ) -> Result<ConnectionIdentity, TargetAuthorityError> {
        match &self.access {
            TargetServerAccess::Hosted(sessions) => {
                let bearer = head
                    .bearer()
                    .map_err(|()| TargetAuthorityError::unauthorized())?;
                match purpose {
                    BearerPurpose::Control => sessions.authenticate_control(bearer).await,
                    BearerPurpose::Oecp => sessions.authenticate_oecp(bearer).await,
                }
            }
            TargetServerAccess::Private { access, identity } => {
                let bearer = head
                    .bearer()
                    .map_err(|()| TargetAuthorityError::unauthorized())?;
                access.authenticate(bearer).await?;
                Ok(identity.clone())
            }
            TargetServerAccess::Direct(identity) => Ok(identity.clone()),
        }
    }
}

fn is_operator_diagnostics_path(path: &str) -> bool {
    path.starts_with(OPERATOR_DIAGNOSTICS_PATH_PREFIX)
}

fn operator_diagnostics_run_id(path: &str) -> Option<openengine_cluster_protocol::RunId> {
    let value = path.strip_prefix(OPERATOR_DIAGNOSTICS_PATH_PREFIX)?;
    let run_id = openengine_cluster_protocol::RunId::new(value);
    is_canonical_uuid_v7(&run_id).then_some(run_id)
}

async fn read_request_head(stream: &TcpStream) -> io::Result<RequestHead> {
    tokio::time::timeout(REQUEST_HEAD_TIMEOUT, peek_request_head(stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "request headers timed out"))?
}

async fn write_request_error(stream: &mut TcpStream, error: io::Error) -> io::Result<()> {
    match error.kind() {
        io::ErrorKind::InvalidData => {
            write_http_response(
                stream,
                invalid_request_response("target HTTP request is malformed"),
            )
            .await
        }
        io::ErrorKind::TimedOut => write_http_response(stream, request_timeout_response()).await,
        _ => Err(error),
    }
}

#[derive(Clone, Copy)]
enum BearerPurpose {
    Control,
    Oecp,
}

/// Target OECP is primarily an observation/control surface. HTTP submission is the only route that
/// accepts an initial caller-assigned run identity, exact source, and bounded environment. Recovery
/// also accepts the fresh successor identity used to create a linked run.
struct TargetOecpBackend {
    controller: Arc<NativeV2CloudController>,
    workspace_recovery: bool,
    workspace_checkpoints: bool,
}

#[async_trait]
impl ClusterBackend for TargetOecpBackend {
    async fn initialize(
        &self,
        context: &ConnectionContext,
        params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        ClusterBackend::initialize(self.controller.as_ref(), context, params).await
    }

    async fn get(
        &self,
        context: &ConnectionContext,
        params: GetParams,
    ) -> Result<GetResult, BackendError> {
        ClusterBackend::get(self.controller.as_ref(), context, params).await
    }

    async fn run_submit(
        &self,
        _context: &ConnectionContext,
        _params: RunSubmitParams,
    ) -> Result<RunSubmitResult, BackendError> {
        Err(BackendError::application(
            RUN_CONFLICT,
            "target OECP does not accept run submissions",
            None,
        ))
    }

    async fn run_list(
        &self,
        context: &ConnectionContext,
        params: RunListParams,
    ) -> Result<RunListResult, BackendError> {
        ClusterBackend::run_list(self.controller.as_ref(), context, params).await
    }

    async fn run_status(
        &self,
        context: &ConnectionContext,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, BackendError> {
        ClusterBackend::run_status(self.controller.as_ref(), context, params).await
    }

    async fn run_watch(
        &self,
        context: &ConnectionContext,
        params: RunWatchParams,
    ) -> Result<(RunWatchResult, RunWatchEventStream), BackendError> {
        ClusterBackend::run_watch(self.controller.as_ref(), context, params).await
    }

    async fn run_logs(
        &self,
        context: &ConnectionContext,
        params: RunLogsParams,
    ) -> Result<(RunLogsResult, RunLogEventStream), BackendError> {
        ClusterBackend::run_logs(self.controller.as_ref(), context, params).await
    }

    async fn run_attach(
        &self,
        context: &ConnectionContext,
        params: RunAttachParams,
    ) -> Result<(RunAttachResult, RunAttachEventStream), BackendError> {
        ClusterBackend::run_attach(self.controller.as_ref(), context, params).await
    }

    async fn run_force(
        &self,
        context: &ConnectionContext,
        params: RunForceParams,
    ) -> Result<RunForceResult, BackendError> {
        ClusterBackend::run_force(self.controller.as_ref(), context, params).await
    }

    async fn run_checkpoints(
        &self,
        context: &ConnectionContext,
        params: RunCheckpointsParams,
    ) -> Result<RunCheckpointsResult, BackendError> {
        if !self.workspace_checkpoints {
            return Err(workspace_checkpoints_unavailable());
        }
        if !is_canonical_uuid_v7(&params.run_id) {
            return Err(workspace_recovery_invalid_run_id());
        }
        ClusterBackend::run_checkpoints(self.controller.as_ref(), context, params).await
    }

    async fn run_resume(
        &self,
        context: &ConnectionContext,
        params: RunResumeParams,
    ) -> Result<RunResumeResult, BackendError> {
        if !self.workspace_recovery {
            return Err(workspace_recovery_unavailable());
        }
        if params.from.is_some() && !self.workspace_checkpoints {
            return Err(workspace_checkpoints_unavailable());
        }
        if !is_canonical_uuid_v7(&params.run_id) || !is_canonical_uuid_v7(&params.successor_run_id)
        {
            return Err(workspace_recovery_invalid_run_id());
        }
        ClusterBackend::run_resume(self.controller.as_ref(), context, params).await
    }

    async fn run_discard_workspace(
        &self,
        context: &ConnectionContext,
        params: RunDiscardWorkspaceParams,
    ) -> Result<RunDiscardWorkspaceResult, BackendError> {
        if !self.workspace_recovery {
            return Err(workspace_recovery_unavailable());
        }
        if !is_canonical_uuid_v7(&params.run_id) {
            return Err(workspace_recovery_invalid_run_id());
        }
        ClusterBackend::run_discard_workspace(self.controller.as_ref(), context, params).await
    }
}

fn workspace_checkpoints_unavailable() -> BackendError {
    BackendError::application(
        INVALID_PHASE,
        "Backend does not support native-v2 workspace checkpoints",
        None,
    )
}

fn workspace_recovery_unavailable() -> BackendError {
    BackendError::application(
        INVALID_PHASE,
        "Backend does not support native-v2 workspace recovery",
        None,
    )
}

fn workspace_recovery_invalid_run_id() -> BackendError {
    BackendError::invalid_params(
        SCHEMA_VIOLATION,
        "workspace recovery run IDs must be canonical UUIDv7 values",
        None,
    )
}

fn validate_oecp_endpoint(endpoint: &str) -> Result<(), TargetAuthorityError> {
    let url = Url::parse(endpoint)
        .map_err(|_| TargetAuthorityError::invalid("OECP endpoint must be an absolute URL"))?;
    if !matches!(url.scheme(), "ws" | "wss")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != OECP_PATH
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(TargetAuthorityError::invalid(
            "OECP endpoint must be an authority URL ending in /native-v2/oecp",
        ));
    }
    Ok(())
}

fn valid_issued_bearer(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_BEARER_BYTES
        && value.bytes().all(|byte| byte.is_ascii_graphic())
}
