use std::path::PathBuf;
use std::time::Duration;

use crate::protocol::{
    ClientCapabilities, ClientInfo, ClusterLogLinesParams, ClusterTimelineEventsParams, RpcError,
    ServerCapabilities,
};

pub mod framing;
pub mod stdio;

pub const DEFAULT_PROTOCOL_VERSION: i64 = 1;
pub const BACKEND_PATH_ENV: &str = "ZEROSHOT_TUI_BACKEND_PATH";
pub const DEFAULT_BACKEND_RELATIVE_PATH: &str = "lib/tui-backend/server.js";

#[derive(Debug, Clone)]
pub struct BackendConfig {
    pub backend_path: Option<PathBuf>,
    pub protocol_version: i64,
    pub client: ClientInfo,
    pub capabilities: Option<ClientCapabilities>,
    pub request_timeout: Option<Duration>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        let client = ClientInfo {
            name: "zeroshot-tui".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: Some(std::process::id() as i64),
        };
        let backend_path = std::env::var(BACKEND_PATH_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from);
        Self {
            backend_path,
            protocol_version: DEFAULT_PROTOCOL_VERSION,
            client,
            capabilities: None,
            request_timeout: Some(Duration::from_secs(30)),
        }
    }
}

impl BackendConfig {
    pub fn with_backend_path(path: impl Into<PathBuf>) -> Self {
        let mut config = Self::default();
        config.backend_path = Some(path.into());
        config
    }
}

#[derive(Debug, Clone)]
pub struct BackendExit {
    pub code: Option<i32>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum BackendNotification {
    ClusterLogLines(ClusterLogLinesParams),
    ClusterTimelineEvents(ClusterTimelineEventsParams),
    Unknown {
        method: String,
        params: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone)]
pub enum BackendEvent {
    Notification(BackendNotification),
    BackendExited(BackendExit),
}

#[derive(Debug)]
pub enum BackendError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Frame(framing::FrameError),
    Rpc(RpcError),
    Protocol(String),
    Disconnected(String),
    Timeout(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Io(err) => write!(f, "IO error: {err}"),
            BackendError::Json(err) => write!(f, "JSON error: {err}"),
            BackendError::Frame(err) => write!(f, "Frame error: {err}"),
            BackendError::Rpc(err) => write!(f, "RPC error {}: {}", err.code, err.message),
            BackendError::Protocol(message) => write!(f, "Protocol error: {message}"),
            BackendError::Disconnected(message) => write!(f, "Backend disconnected: {message}"),
            BackendError::Timeout(message) => write!(f, "Request timeout: {message}"),
        }
    }
}

impl std::error::Error for BackendError {}

impl From<std::io::Error> for BackendError {
    fn from(err: std::io::Error) -> Self {
        BackendError::Io(err)
    }
}

impl From<serde_json::Error> for BackendError {
    fn from(err: serde_json::Error) -> Self {
        BackendError::Json(err)
    }
}

impl From<framing::FrameError> for BackendError {
    fn from(err: framing::FrameError) -> Self {
        BackendError::Frame(err)
    }
}

pub trait BackendClient {
    fn take_event_receiver(&mut self) -> Option<std::sync::mpsc::Receiver<BackendEvent>>;
    fn server_capabilities(&self) -> Option<ServerCapabilities>;
    fn protocol_version(&self) -> i64;
    fn shutdown(&mut self) -> Result<(), BackendError>;
}
