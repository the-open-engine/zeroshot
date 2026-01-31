use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::backend::framing::FrameDecoder;
use crate::backend::{encode_frame, BackendClient, BackendError, BackendEvent, BackendNotification, PROTOCOL_VERSION};
use crate::protocol;

const BACKEND_PATH_ENV: &str = "ZEROSHOT_TUI_BACKEND_PATH";
const REQUIRED_METHODS: [&str; 9] = [
    "initialize",
    "listClusters",
    "getClusterSummary",
    "subscribeClusterLogs",
    "subscribeClusterTimeline",
    "startClusterFromText",
    "startClusterFromIssue",
    "sendGuidanceToAgent",
    "sendGuidanceToCluster",
];
const REQUIRED_NOTIFICATIONS: [&str; 2] = ["clusterLogLines", "clusterTimelineEvents"];

pub struct StdioBackendClient {
    writer_tx: mpsc::Sender<OutgoingMessage>,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<Result<serde_json::Value, BackendError>>>>>,
    event_rx: Mutex<mpsc::Receiver<BackendEvent>>,
    next_id: AtomicI64,
    protocol_version: i64,
    capabilities: protocol::ServerCapabilities,
    child: Mutex<Option<Child>>,
    reader_handle: Option<JoinHandle<()>>,
    writer_handle: Option<JoinHandle<()>>,
}

impl StdioBackendClient {
    pub fn connect(params: protocol::InitializeParams) -> Result<Self, BackendError> {
        let backend_path = backend_path();
        if !backend_path.exists() {
            return Err(BackendError::Spawn(format!(
                "backend path not found: {}",
                backend_path.display()
            )));
        }

        let mut child = Command::new("node")
            .arg(backend_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(BackendError::Io)?;

        let stdin = child.stdin.take().ok_or_else(|| {
            BackendError::Spawn("failed to capture backend stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            BackendError::Spawn("failed to capture backend stdout".to_string())
        })?;

        let (writer_tx, writer_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let pending = Arc::new(Mutex::new(HashMap::new()));

        let reader_pending = Arc::clone(&pending);
        let reader_event = event_tx.clone();
        let reader_handle = Some(thread::spawn(move || {
            reader_loop(stdout, reader_pending, reader_event);
        }));

        let writer_pending = Arc::clone(&pending);
        let writer_event = event_tx.clone();
        let writer_handle = Some(thread::spawn(move || {
            writer_loop(stdin, writer_rx, writer_pending, writer_event);
        }));

        let mut client = StdioBackendClient {
            writer_tx,
            pending,
            event_rx: Mutex::new(event_rx),
            next_id: AtomicI64::new(1),
            protocol_version: PROTOCOL_VERSION,
            capabilities: protocol::ServerCapabilities {
                methods: Vec::new(),
                notifications: Vec::new(),
            },
            child: Mutex::new(Some(child)),
            reader_handle,
            writer_handle,
        };

        let init = client.initialize(params)?;
        if init.protocol_version != PROTOCOL_VERSION {
            let mismatch = BackendError::ProtocolVersionMismatch {
                expected: PROTOCOL_VERSION,
                got: init.protocol_version,
            };
            if let Err(err) = client.shutdown() {
                return Err(BackendError::BackendClosed(format!(
                    "{mismatch}; shutdown error: {err}"
                )));
            }
            return Err(mismatch);
        }
        if let Err(err) = validate_capabilities(&init.capabilities) {
            if let Err(shutdown_err) = client.shutdown() {
                return Err(BackendError::BackendClosed(format!(
                    "{err}; shutdown error: {shutdown_err}"
                )));
            }
            return Err(err);
        }
        client.protocol_version = init.protocol_version;
        client.capabilities = init.capabilities;

        Ok(client)
    }

    pub fn connect_default() -> Result<Self, BackendError> {
        let params = default_initialize_params();
        Self::connect(params)
    }

    fn initialize(&self, params: protocol::InitializeParams) -> Result<protocol::InitializeResult, BackendError> {
        self.send_request("initialize", Some(params))
    }

    fn send_request<T: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<T>,
    ) -> Result<R, BackendError> {
        let id = protocol::JsonRpcId::Number(self.next_id.fetch_add(1, Ordering::SeqCst));
        let request = protocol::JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: id.clone(),
            method: method.to_string(),
            params,
        };

        let payload = serde_json::to_vec(&request)?;
        let frame = encode_frame(&payload)?;

        let (tx, rx) = mpsc::channel();
        {
            let mut pending = self.pending.lock().map_err(|_| {
                BackendError::Send("pending request lock poisoned".to_string())
            })?;
            pending.insert(id_key(&id), tx);
        }

        if self.writer_tx.send(OutgoingMessage::Frame(frame)).is_err() {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id_key(&id));
            }
            return Err(BackendError::Send("failed to send request".to_string()));
        }

        let response = rx
            .recv()
            .map_err(|_| BackendError::BackendClosed("response channel closed".to_string()))??;

        let parsed = serde_json::from_value(response)?;
        Ok(parsed)
    }

    // Notifications from the client are not used in the MVP flow.
}

impl BackendClient for StdioBackendClient {
    fn protocol_version(&self) -> i64 {
        self.protocol_version
    }

    fn capabilities(&self) -> &protocol::ServerCapabilities {
        &self.capabilities
    }

    fn try_next_notification(&self) -> Result<Option<BackendNotification>, BackendError> {
        let rx = self
            .event_rx
            .lock()
            .map_err(|_| BackendError::BackendClosed("event lock poisoned".to_string()))?;
        match rx.try_recv() {
            Ok(event) => match event {
                BackendEvent::Notification(notification) => Ok(Some(notification)),
                BackendEvent::Closed(err) => Err(err),
            },
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(BackendError::BackendClosed("event channel closed".to_string()))
            }
        }
    }

    fn recv_notification(&self) -> Result<BackendNotification, BackendError> {
        let rx = self
            .event_rx
            .lock()
            .map_err(|_| BackendError::BackendClosed("event lock poisoned".to_string()))?;
        match rx.recv() {
            Ok(event) => match event {
                BackendEvent::Notification(notification) => Ok(notification),
                BackendEvent::Closed(err) => Err(err),
            },
            Err(_) => Err(BackendError::BackendClosed("event channel closed".to_string())),
        }
    }

    fn list_clusters(&self) -> Result<protocol::ListClustersResult, BackendError> {
        self.send_request("listClusters", Option::<protocol::ListClustersParams>::None)
    }

    fn get_cluster_summary(
        &self,
        params: protocol::GetClusterSummaryParams,
    ) -> Result<protocol::GetClusterSummaryResult, BackendError> {
        self.send_request("getClusterSummary", Some(params))
    }

    fn subscribe_cluster_logs(
        &self,
        params: protocol::SubscribeClusterLogsParams,
    ) -> Result<protocol::SubscribeResult, BackendError> {
        self.send_request("subscribeClusterLogs", Some(params))
    }

    fn subscribe_cluster_timeline(
        &self,
        params: protocol::SubscribeClusterTimelineParams,
    ) -> Result<protocol::SubscribeResult, BackendError> {
        self.send_request("subscribeClusterTimeline", Some(params))
    }

    fn start_cluster_from_text(
        &self,
        params: protocol::StartClusterFromTextParams,
    ) -> Result<protocol::StartClusterResult, BackendError> {
        self.send_request("startClusterFromText", Some(params))
    }

    fn start_cluster_from_issue(
        &self,
        params: protocol::StartClusterFromIssueParams,
    ) -> Result<protocol::StartClusterResult, BackendError> {
        self.send_request("startClusterFromIssue", Some(params))
    }

    fn send_guidance_to_agent(
        &self,
        params: protocol::SendGuidanceToAgentParams,
    ) -> Result<protocol::SendGuidanceToAgentResult, BackendError> {
        self.send_request("sendGuidanceToAgent", Some(params))
    }

    fn send_guidance_to_cluster(
        &self,
        params: protocol::SendGuidanceToClusterParams,
    ) -> Result<protocol::SendGuidanceToClusterResult, BackendError> {
        self.send_request("sendGuidanceToCluster", Some(params))
    }

    fn shutdown(&self) -> Result<(), BackendError> {
        let mut errors = Vec::new();
        if self.writer_tx.send(OutgoingMessage::Shutdown).is_err() {
            errors.push("failed to send shutdown to writer".to_string());
        }
        let mut child_lock = self
            .child
            .lock()
            .map_err(|_| BackendError::BackendClosed("child lock poisoned".to_string()))?;
        if let Some(child) = child_lock.as_mut() {
            if let Err(err) = child.kill() {
                errors.push(format!("failed to kill backend: {err}"));
            }
            if let Err(err) = child.wait() {
                errors.push(format!("failed to wait for backend: {err}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(BackendError::BackendClosed(format!(
                "shutdown errors: {}",
                errors.join("; ")
            )))
        }
    }
}

impl Drop for StdioBackendClient {
    fn drop(&mut self) {
        if let Err(err) = self.shutdown() {
            eprintln!("backend shutdown error: {err}");
        }
        if let Some(handle) = self.reader_handle.take() {
            if let Err(err) = handle.join() {
                eprintln!("backend reader thread join error: {:?}", err);
            }
        }
        if let Some(handle) = self.writer_handle.take() {
            if let Err(err) = handle.join() {
                eprintln!("backend writer thread join error: {:?}", err);
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    id: Option<protocol::JsonRpcId>,
    method: Option<String>,
    params: Option<serde_json::Value>,
    result: Option<serde_json::Value>,
    error: Option<protocol::RpcError>,
}

enum OutgoingMessage {
    Frame(Vec<u8>),
    Shutdown,
}

fn backend_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var(BACKEND_PATH_ENV) {
        if !path.trim().is_empty() {
            return std::path::PathBuf::from(path);
        }
    }

    let cwd_default = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("lib/tui-backend/server.js");
    if cwd_default.exists() {
        return cwd_default;
    }

    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.join("../../..");
    let repo_default = repo_root.join("lib/tui-backend/server.js");
    if repo_default.exists() {
        return repo_default;
    }

    std::path::PathBuf::from("lib/tui-backend/server.js")
}

fn validate_capabilities(
    capabilities: &protocol::ServerCapabilities,
) -> Result<(), BackendError> {
    let method_set: HashSet<&str> = capabilities.methods.iter().map(|m| m.as_str()).collect();
    let notification_set: HashSet<&str> = capabilities
        .notifications
        .iter()
        .map(|n| n.as_str())
        .collect();

    let mut missing_methods = Vec::new();
    for required in REQUIRED_METHODS {
        if !method_set.contains(required) {
            missing_methods.push(required.to_string());
        }
    }

    let mut missing_notifications = Vec::new();
    for required in REQUIRED_NOTIFICATIONS {
        if !notification_set.contains(required) {
            missing_notifications.push(required.to_string());
        }
    }

    if missing_methods.is_empty() && missing_notifications.is_empty() {
        Ok(())
    } else {
        Err(BackendError::UnsupportedCapability {
            missing_methods,
            missing_notifications,
        })
    }
}

fn default_initialize_params() -> protocol::InitializeParams {
    protocol::InitializeParams {
        protocol_version: PROTOCOL_VERSION,
        client: protocol::ClientInfo {
            name: "zeroshot-tui".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: Some(std::process::id() as i64),
        },
        capabilities: Some(protocol::ClientCapabilities {
            wants_metrics: Some(true),
            wants_topology: Some(false),
        }),
    }
}

fn reader_loop(
    mut stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<Result<serde_json::Value, BackendError>>>>>,
    event_tx: mpsc::Sender<BackendEvent>,
) {
    let mut decoder = FrameDecoder::new();
    let mut buffer = [0u8; 8192];

    loop {
        let read_count = match stdout.read(&mut buffer) {
            Ok(0) => {
                notify_backend_closed(
                    BackendError::BackendClosed("backend stdout closed".to_string()),
                    &pending,
                    &event_tx,
                );
                break;
            }
            Ok(count) => count,
            Err(err) => {
                notify_backend_closed(BackendError::Io(err), &pending, &event_tx);
                break;
            }
        };

        let frames = match decoder.push_bytes(&buffer[..read_count]) {
            Ok(frames) => frames,
            Err(err) => {
                notify_backend_closed(err, &pending, &event_tx);
                break;
            }
        };

        for frame in frames {
            if let Err(err) = handle_frame(&frame, &pending, &event_tx) {
                notify_backend_closed(err, &pending, &event_tx);
                return;
            }
        }
    }
}

fn handle_frame(
    frame: &[u8],
    pending: &Arc<Mutex<HashMap<String, mpsc::Sender<Result<serde_json::Value, BackendError>>>>>,
    event_tx: &mpsc::Sender<BackendEvent>,
) -> Result<(), BackendError> {
    let raw: RawMessage = serde_json::from_slice(frame)?;
    dispatch_raw_message(raw, pending, event_tx)
}

fn dispatch_raw_message(
    raw: RawMessage,
    pending: &Arc<Mutex<HashMap<String, mpsc::Sender<Result<serde_json::Value, BackendError>>>>>,
    event_tx: &mpsc::Sender<BackendEvent>,
) -> Result<(), BackendError> {
    if let Some(id) = raw.id {
        let key = id_key(&id);
        let mut pending = pending
            .lock()
            .map_err(|_| BackendError::BackendClosed("pending lock poisoned".to_string()))?;
        if let Some(tx) = pending.remove(&key) {
            if let Some(error) = raw.error {
                let _ = tx.send(Err(BackendError::Rpc(error)));
            } else if let Some(result) = raw.result {
                let _ = tx.send(Ok(result));
            } else {
                let _ = tx.send(Err(BackendError::InvalidResponse(
                    "missing result or error".to_string(),
                )));
            }
            return Ok(());
        }
        return Err(BackendError::InvalidResponse(format!(
            "response id not pending: {key}"
        )));
    }

    if let Some(method) = raw.method {
        let params_value = raw.params.unwrap_or(serde_json::Value::Null);
        let notification = match method.as_str() {
            "clusterLogLines" => {
                let params: protocol::ClusterLogLinesParams = serde_json::from_value(params_value)?;
                BackendNotification::ClusterLogLines(params)
            }
            "clusterTimelineEvents" => {
                let params: protocol::ClusterTimelineEventsParams =
                    serde_json::from_value(params_value)?;
                BackendNotification::ClusterTimelineEvents(params)
            }
            _ => BackendNotification::Unknown {
                method,
                params: params_value,
            },
        };

        event_tx
            .send(BackendEvent::Notification(notification))
            .map_err(|_| BackendError::Send("notification channel closed".to_string()))?;
        return Ok(());
    }

    Err(BackendError::InvalidResponse(
        "message missing id and method".to_string(),
    ))
}

fn writer_loop(
    stdin: ChildStdin,
    rx: mpsc::Receiver<OutgoingMessage>,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<Result<serde_json::Value, BackendError>>>>>,
    event_tx: mpsc::Sender<BackendEvent>,
) {
    let mut writer = std::io::BufWriter::new(stdin);
    while let Ok(message) = rx.recv() {
        match message {
            OutgoingMessage::Frame(frame) => {
                if writer.write_all(&frame).is_err() || writer.flush().is_err() {
                    notify_backend_closed(
                        BackendError::BackendClosed("backend stdin closed".to_string()),
                        &pending,
                        &event_tx,
                    );
                    break;
                }
            }
            OutgoingMessage::Shutdown => {
                let _ = writer.flush();
                break;
            }
        }
    }
}

fn notify_backend_closed(
    err: BackendError,
    pending: &Arc<Mutex<HashMap<String, mpsc::Sender<Result<serde_json::Value, BackendError>>>>>,
    event_tx: &mpsc::Sender<BackendEvent>,
) {
    let message = err.to_string();
    if let Ok(mut pending) = pending.lock() {
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(BackendError::BackendClosed(message.clone())));
        }
    }
    let _ = event_tx.send(BackendEvent::Closed(BackendError::BackendClosed(message)));
}

fn id_key(id: &protocol::JsonRpcId) -> String {
    match id {
        protocol::JsonRpcId::String(value) => format!("s:{value}"),
        protocol::JsonRpcId::Number(value) => format!("n:{value}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_interleaved_notification_and_response() {
        let (event_tx, event_rx) = mpsc::channel();
        let pending: Arc<Mutex<HashMap<String, mpsc::Sender<Result<serde_json::Value, BackendError>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (resp_tx, resp_rx) = mpsc::channel();
        let request_id = protocol::JsonRpcId::Number(42);
        pending
            .lock()
            .expect("lock")
            .insert(id_key(&request_id), resp_tx);

        let notification = RawMessage {
            id: None,
            method: Some("clusterLogLines".to_string()),
            params: Some(serde_json::json!({
                "subscriptionId": "sub-1",
                "clusterId": "cluster-1",
                "lines": [],
                "droppedCount": 0
            })),
            result: None,
            error: None,
        };

        let response = RawMessage {
            id: Some(request_id),
            method: None,
            params: None,
            result: Some(serde_json::json!({ "clusters": [] })),
            error: None,
        };

        dispatch_raw_message(notification, &pending, &event_tx).expect("notification");
        dispatch_raw_message(response, &pending, &event_tx).expect("response");

        let event = event_rx.recv().expect("event");
        match event {
            BackendEvent::Notification(BackendNotification::ClusterLogLines(_)) => {}
            other => panic!("unexpected event: {other:?}"),
        }

        let result = resp_rx.recv().expect("response channel");
        let value = result.expect("response ok");
        let parsed: protocol::ListClustersResult = serde_json::from_value(value).expect("parse");
        assert_eq!(parsed.clusters.len(), 0);
    }
}
