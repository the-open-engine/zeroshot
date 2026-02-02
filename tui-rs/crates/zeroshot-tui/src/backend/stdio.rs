use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::backend::framing::{FrameDecoder, FrameEncoder};
use crate::backend::{
    BackendClient, BackendConfig, BackendError, BackendEvent, BackendExit, BackendNotification,
    BACKEND_PATH_ENV, DEFAULT_BACKEND_RELATIVE_PATH,
};
use crate::protocol::{
    ClientCapabilities, ClientInfo, ClusterLogLinesParams, ClusterTimelineEventsParams,
    GetClusterSummaryParams, GetClusterSummaryResult, GetClusterTopologyParams,
    GetClusterTopologyResult, InitializeParams, InitializeResult, JsonRpcId, JsonRpcRequest,
    ListClusterMetricsParams, ListClusterMetricsResult, ListClustersParams, ListClustersResult,
    SendGuidanceToAgentParams, SendGuidanceToAgentResult, SendGuidanceToClusterParams,
    SendGuidanceToClusterResult, ServerCapabilities, StartClusterFromIssueParams,
    StartClusterFromTextParams, StartClusterResult, SubscribeClusterLogsParams,
    SubscribeClusterTimelineParams, SubscribeResult, UnsubscribeParams, UnsubscribeResult,
};

const JSONRPC_VERSION: &str = "2.0";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RequestKey {
    Number(i64),
    String(String),
}

impl RequestKey {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_i64().map(RequestKey::Number),
            Value::String(value) => Some(RequestKey::String(value.clone())),
            _ => None,
        }
    }
}

enum WriterCommand {
    Frame(Vec<u8>),
    Shutdown,
}

pub struct StdioBackendClient {
    config: BackendConfig,
    writer: Option<mpsc::Sender<WriterCommand>>,
    events: Option<mpsc::Receiver<BackendEvent>>,
    pending: Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>>,
    next_id: AtomicI64,
    read_handle: Option<thread::JoinHandle<()>>,
    write_handle: Option<thread::JoinHandle<()>>,
    child: Arc<Mutex<Option<Child>>>,
    protocol_version: i64,
    server_capabilities: Option<ServerCapabilities>,
}

impl StdioBackendClient {
    pub fn connect(config: BackendConfig) -> Result<Self, BackendError> {
        let backend_path = resolve_backend_path(&config)?;
        let mut command = Command::new("node");
        command.arg(backend_path);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::inherit());
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| BackendError::Protocol("Failed to capture backend stdout".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| BackendError::Protocol("Failed to capture backend stdin".into()))?;

        let (event_tx, event_rx) = mpsc::channel();
        let (writer_tx, writer_rx) = mpsc::channel();
        let pending: Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let child = Arc::new(Mutex::new(Some(child)));

        let write_handle = spawn_writer_thread(stdin, writer_rx, pending.clone(), event_tx.clone());
        let read_handle = spawn_reader_thread(stdout, pending.clone(), event_tx.clone());

        let mut client = Self {
            config,
            writer: Some(writer_tx),
            events: Some(event_rx),
            pending,
            next_id: AtomicI64::new(1),
            read_handle: Some(read_handle),
            write_handle: Some(write_handle),
            child,
            protocol_version: 0,
            server_capabilities: None,
        };

        let initialize = client.initialize()?;
        client.validate_initialize(&initialize)?;
        client.protocol_version = initialize.protocol_version;
        client.server_capabilities = Some(initialize.capabilities.clone());

        Ok(client)
    }

    pub fn list_clusters(&self) -> Result<ListClustersResult, BackendError> {
        self.send_request("listClusters", ListClustersParams {})
    }

    pub fn list_cluster_metrics(
        &self,
        params: ListClusterMetricsParams,
    ) -> Result<ListClusterMetricsResult, BackendError> {
        self.send_request("listClusterMetrics", params)
    }

    pub fn get_cluster_summary(
        &self,
        params: GetClusterSummaryParams,
    ) -> Result<GetClusterSummaryResult, BackendError> {
        self.send_request("getClusterSummary", params)
    }

    pub fn get_cluster_topology(
        &self,
        params: GetClusterTopologyParams,
    ) -> Result<GetClusterTopologyResult, BackendError> {
        self.send_request("getClusterTopology", params)
    }

    pub fn subscribe_cluster_logs(
        &self,
        params: SubscribeClusterLogsParams,
    ) -> Result<SubscribeResult, BackendError> {
        self.send_request("subscribeClusterLogs", params)
    }

    pub fn subscribe_cluster_timeline(
        &self,
        params: SubscribeClusterTimelineParams,
    ) -> Result<SubscribeResult, BackendError> {
        self.send_request("subscribeClusterTimeline", params)
    }

    pub fn unsubscribe(
        &self,
        params: UnsubscribeParams,
    ) -> Result<UnsubscribeResult, BackendError> {
        self.send_request("unsubscribe", params)
    }

    pub fn start_cluster_from_text(
        &self,
        params: StartClusterFromTextParams,
    ) -> Result<StartClusterResult, BackendError> {
        self.send_request("startClusterFromText", params)
    }

    pub fn start_cluster_from_issue(
        &self,
        params: StartClusterFromIssueParams,
    ) -> Result<StartClusterResult, BackendError> {
        self.send_request("startClusterFromIssue", params)
    }

    pub fn send_guidance_to_agent(
        &self,
        params: SendGuidanceToAgentParams,
    ) -> Result<SendGuidanceToAgentResult, BackendError> {
        self.send_request("sendGuidanceToAgent", params)
    }

    pub fn send_guidance_to_cluster(
        &self,
        params: SendGuidanceToClusterParams,
    ) -> Result<SendGuidanceToClusterResult, BackendError> {
        self.send_request("sendGuidanceToCluster", params)
    }

    pub fn client_info(&self) -> &ClientInfo {
        &self.config.client
    }

    pub fn client_capabilities(&self) -> Option<&ClientCapabilities> {
        self.config.capabilities.as_ref()
    }

    fn initialize(&self) -> Result<InitializeResult, BackendError> {
        let params = InitializeParams {
            protocol_version: self.config.protocol_version,
            client: self.config.client.clone(),
            capabilities: self.config.capabilities.clone(),
        };
        self.send_request("initialize", params)
    }

    fn validate_initialize(&self, initialize: &InitializeResult) -> Result<(), BackendError> {
        if initialize.protocol_version != self.config.protocol_version {
            return Err(BackendError::Protocol(format!(
                "Protocol version mismatch: expected {}, got {}",
                self.config.protocol_version, initialize.protocol_version
            )));
        }
        Ok(())
    }

    fn send_request<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, BackendError> {
        let (key, frame, rx) = self.prepare_request(method, params)?;
        self.send_frame(&key, frame)?;
        let response = self.await_response(method, &key, rx)?;
        Ok(serde_json::from_value(response)?)
    }

    fn prepare_request<P: Serialize>(
        &self,
        method: &str,
        params: P,
    ) -> Result<
        (
            RequestKey,
            Vec<u8>,
            mpsc::Receiver<Result<Value, BackendError>>,
        ),
        BackendError,
    > {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: JsonRpcId::Number(id),
            method: method.to_string(),
            params: Some(params),
        };
        let payload = serde_json::to_vec(&request)?;
        let frame = FrameEncoder::encode(&payload)?;
        let (tx, rx) = mpsc::channel();
        let key = RequestKey::Number(id);
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| BackendError::Protocol("Pending request lock poisoned".into()))?;
        pending.insert(key.clone(), tx);
        Ok((key, frame, rx))
    }

    fn send_frame(&self, key: &RequestKey, frame: Vec<u8>) -> Result<(), BackendError> {
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| BackendError::Disconnected("Backend writer closed".into()))?;
        if writer.send(WriterCommand::Frame(frame)).is_err() {
            self.remove_pending(key)?;
            return Err(BackendError::Disconnected(
                "Backend writer channel closed".into(),
            ));
        }
        Ok(())
    }

    fn await_response(
        &self,
        method: &str,
        key: &RequestKey,
        rx: mpsc::Receiver<Result<Value, BackendError>>,
    ) -> Result<Value, BackendError> {
        let response = match self.config.request_timeout {
            Some(timeout) => match rx.recv_timeout(timeout) {
                Ok(value) => value,
                Err(err) => {
                    self.remove_pending(key)?;
                    return Err(BackendError::Timeout(format!("{method} failed: {err}")));
                }
            },
            None => match rx.recv() {
                Ok(value) => value,
                Err(_) => {
                    self.remove_pending(key)?;
                    return Err(BackendError::Disconnected("Backend disconnected".into()));
                }
            },
        }?;
        Ok(response)
    }

    fn remove_pending(&self, key: &RequestKey) -> Result<(), BackendError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| BackendError::Protocol("Pending request lock poisoned".into()))?;
        pending.remove(key);
        Ok(())
    }
}

impl BackendClient for StdioBackendClient {
    fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<BackendEvent>> {
        self.events.take()
    }

    fn server_capabilities(&self) -> Option<ServerCapabilities> {
        self.server_capabilities.clone()
    }

    fn protocol_version(&self) -> i64 {
        self.protocol_version
    }

    fn shutdown(&mut self) -> Result<(), BackendError> {
        if let Some(writer) = self.writer.take() {
            let _ = writer.send(WriterCommand::Shutdown);
        }

        if let Some(mut child) = self
            .child
            .lock()
            .map_err(|_| BackendError::Protocol("Child lock poisoned".into()))?
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }

        if let Some(handle) = self.write_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.read_handle.take() {
            let _ = handle.join();
        }
        Ok(())
    }
}

impl Drop for StdioBackendClient {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn spawn_writer_thread(
    mut stdin: impl Write + Send + 'static,
    receiver: mpsc::Receiver<WriterCommand>,
    pending: Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>>,
    event_tx: mpsc::Sender<BackendEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || writer_loop(&mut stdin, receiver, pending, event_tx))
}

fn writer_loop(
    stdin: &mut (impl Write + Send + 'static),
    receiver: mpsc::Receiver<WriterCommand>,
    pending: Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>>,
    event_tx: mpsc::Sender<BackendEvent>,
) {
    for command in receiver {
        match command {
            WriterCommand::Frame(frame) => {
                if let Err(err) = stdin.write_all(&frame) {
                    let error = BackendError::Io(err);
                    let message = error.to_string();
                    drain_pending(&pending, error);
                    let _ = event_tx.send(BackendEvent::BackendExited(BackendExit {
                        code: None,
                        message,
                    }));
                    break;
                }
                let _ = stdin.flush();
            }
            WriterCommand::Shutdown => break,
        }
    }
}

fn spawn_reader_thread(
    mut stdout: impl Read + Send + 'static,
    pending: Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>>,
    event_tx: mpsc::Sender<BackendEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || reader_loop(&mut stdout, pending, event_tx))
}

fn reader_loop(
    stdout: &mut (impl Read + Send + 'static),
    pending: Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>>,
    event_tx: mpsc::Sender<BackendEvent>,
) {
    let mut decoder = FrameDecoder::new();
    let mut buffer = [0u8; 8192];
    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => {
                handle_reader_disconnect(
                    &pending,
                    &event_tx,
                    BackendError::Disconnected("Backend closed stdout".into()),
                    "Backend closed stdout",
                );
                break;
            }
            Ok(bytes) => {
                if let Err(err) =
                    handle_reader_bytes(&buffer[..bytes], &mut decoder, &pending, &event_tx)
                {
                    let message = err.to_string();
                    drain_pending(&pending, err);
                    let _ = event_tx.send(BackendEvent::BackendExited(BackendExit {
                        code: None,
                        message,
                    }));
                    return;
                }
            }
            Err(err) => {
                handle_reader_disconnect(
                    &pending,
                    &event_tx,
                    BackendError::Io(err),
                    "Backend stdout read failed",
                );
                break;
            }
        }
    }
}

fn handle_reader_bytes(
    bytes: &[u8],
    decoder: &mut FrameDecoder,
    pending: &Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>>,
    event_tx: &mpsc::Sender<BackendEvent>,
) -> Result<(), BackendError> {
    let frames = decoder.push(bytes)?;
    for frame in frames {
        handle_frame(&frame, pending, event_tx)?;
    }
    Ok(())
}

fn handle_reader_disconnect(
    pending: &Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>>,
    event_tx: &mpsc::Sender<BackendEvent>,
    error: BackendError,
    message: &str,
) {
    let event_message = if matches!(error, BackendError::Disconnected(_)) {
        message.to_string()
    } else {
        error.to_string()
    };
    drain_pending(pending, error);
    let _ = event_tx.send(BackendEvent::BackendExited(BackendExit {
        code: None,
        message: event_message,
    }));
}

fn handle_frame(
    frame: &[u8],
    pending: &Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>>,
    event_tx: &mpsc::Sender<BackendEvent>,
) -> Result<(), BackendError> {
    let value = parse_frame_json(frame)?;
    ensure_jsonrpc_version(&value)?;
    if is_notification(&value) {
        return handle_notification(value, event_tx);
    }
    let key = parse_response_id(&value)?;
    dispatch_response(value, key, pending);
    Ok(())
}

fn parse_frame_json(frame: &[u8]) -> Result<Value, BackendError> {
    let value: Value = serde_json::from_slice(frame)?;
    if !value.is_object() {
        return Err(BackendError::Protocol("Non-object JSON-RPC message".into()));
    }
    Ok(value)
}

fn ensure_jsonrpc_version(value: &Value) -> Result<(), BackendError> {
    let jsonrpc = value
        .get("jsonrpc")
        .and_then(|value| value.as_str())
        .ok_or_else(|| BackendError::Protocol("Missing jsonrpc version".into()))?;
    if jsonrpc != JSONRPC_VERSION {
        return Err(BackendError::Protocol(format!(
            "Unsupported jsonrpc version: {jsonrpc}"
        )));
    }
    Ok(())
}

fn is_notification(value: &Value) -> bool {
    value.get("id").is_none()
}

fn parse_response_id(value: &Value) -> Result<RequestKey, BackendError> {
    let id_value = value
        .get("id")
        .ok_or_else(|| BackendError::Protocol("Missing id".into()))?;
    RequestKey::from_value(id_value).ok_or_else(|| BackendError::Protocol("Invalid id type".into()))
}

fn dispatch_response(
    value: Value,
    key: RequestKey,
    pending: &Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>>,
) {
    let sender = {
        let mut pending = match pending.lock() {
            Ok(lock) => lock,
            Err(poisoned) => poisoned.into_inner(),
        };
        pending.remove(&key)
    };

    if let Some(sender) = sender {
        if let Some(error_value) = value.get("error") {
            let error = parse_rpc_error(error_value);
            let _ = sender.send(Err(BackendError::Rpc(error)));
            return;
        }
        if let Some(result) = value.get("result") {
            let _ = sender.send(Ok(result.clone()));
            return;
        }
        let _ = sender.send(Err(BackendError::Protocol(
            "Response missing result or error".into(),
        )));
    }
}

fn parse_rpc_error(error_value: &Value) -> crate::protocol::RpcError {
    serde_json::from_value(error_value.clone()).unwrap_or_else(|err| crate::protocol::RpcError {
        code: -32603,
        message: format!("Failed to parse RPC error: {err}"),
        data: None,
    })
}

fn handle_notification(
    value: Value,
    event_tx: &mpsc::Sender<BackendEvent>,
) -> Result<(), BackendError> {
    let method = value
        .get("method")
        .and_then(|value| value.as_str())
        .ok_or_else(|| BackendError::Protocol("Notification missing method".into()))?;
    let params = value.get("params").cloned();

    let notification = match method {
        "clusterLogLines" => {
            let params_value = params
                .ok_or_else(|| BackendError::Protocol("clusterLogLines missing params".into()))?;
            let parsed: ClusterLogLinesParams = serde_json::from_value(params_value)?;
            BackendNotification::ClusterLogLines(parsed)
        }
        "clusterTimelineEvents" => {
            let params_value = params.ok_or_else(|| {
                BackendError::Protocol("clusterTimelineEvents missing params".into())
            })?;
            let parsed: ClusterTimelineEventsParams = serde_json::from_value(params_value)?;
            BackendNotification::ClusterTimelineEvents(parsed)
        }
        _ => BackendNotification::Unknown {
            method: method.to_string(),
            params,
        },
    };

    let _ = event_tx.send(BackendEvent::Notification(notification));
    Ok(())
}

fn drain_pending(
    pending: &Arc<Mutex<HashMap<RequestKey, mpsc::Sender<Result<Value, BackendError>>>>>,
    error: BackendError,
) {
    let message = error.to_string();
    let senders = {
        let mut pending = match pending.lock() {
            Ok(lock) => lock,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut items = Vec::with_capacity(pending.len());
        for (_, sender) in pending.drain() {
            items.push(sender);
        }
        items
    };

    for sender in senders {
        let _ = sender.send(Err(BackendError::Disconnected(message.clone())));
    }
}

pub fn resolve_backend_path(config: &BackendConfig) -> Result<PathBuf, BackendError> {
    if let Some(path) = config.backend_path.clone() {
        return Ok(path);
    }

    if let Ok(value) = env::var(BACKEND_PATH_ENV) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }

    let cwd = env::current_dir()?;
    if let Some(found) = find_in_ancestors(&cwd, DEFAULT_BACKEND_RELATIVE_PATH) {
        return Ok(found);
    }

    Err(BackendError::Protocol(format!(
        "Backend path not found (set {BACKEND_PATH_ENV} or build {DEFAULT_BACKEND_RELATIVE_PATH})"
    )))
}

fn find_in_ancestors(start: &Path, relative: &str) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        let candidate = ancestor.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
