use std::error::Error;
use std::fmt;

use crate::protocol;

pub const PROTOCOL_VERSION: i64 = 1;
pub const MAX_FRAME_SIZE: usize = 10 * 1024 * 1024;

#[derive(Debug)]
pub enum BackendError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Frame(String),
    Rpc(protocol::RpcError),
    ProtocolVersionMismatch { expected: i64, got: i64 },
    UnsupportedCapability {
        missing_methods: Vec<String>,
        missing_notifications: Vec<String>,
    },
    BackendClosed(String),
    Spawn(String),
    Send(String),
    InvalidResponse(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendError::Io(err) => write!(f, "io error: {err}"),
            BackendError::Json(err) => write!(f, "json error: {err}"),
            BackendError::Frame(err) => write!(f, "frame error: {err}"),
            BackendError::Rpc(err) => write!(f, "rpc error {code}: {message}", code = err.code, message = err.message),
            BackendError::ProtocolVersionMismatch { expected, got } => {
                write!(f, "protocol version mismatch: expected {expected}, got {got}")
            }
            BackendError::UnsupportedCapability {
                missing_methods,
                missing_notifications,
            } => {
                let mut parts = Vec::new();
                if !missing_methods.is_empty() {
                    parts.push(format!("methods: {}", missing_methods.join(", ")));
                }
                if !missing_notifications.is_empty() {
                    parts.push(format!(
                        "notifications: {}",
                        missing_notifications.join(", ")
                    ));
                }
                write!(
                    f,
                    "unsupported capability: missing {}",
                    parts.join("; ")
                )
            }
            BackendError::BackendClosed(reason) => write!(f, "backend closed: {reason}"),
            BackendError::Spawn(reason) => write!(f, "spawn error: {reason}"),
            BackendError::Send(reason) => write!(f, "send error: {reason}"),
            BackendError::InvalidResponse(reason) => write!(f, "invalid response: {reason}"),
        }
    }
}

impl Error for BackendError {}

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

#[derive(Debug, Clone, PartialEq)]
pub enum BackendNotification {
    ClusterLogLines(protocol::ClusterLogLinesParams),
    ClusterTimelineEvents(protocol::ClusterTimelineEventsParams),
    Unknown { method: String, params: serde_json::Value },
}

#[derive(Debug)]
pub enum BackendEvent {
    Notification(BackendNotification),
    Closed(BackendError),
}

pub trait BackendClient {
    fn protocol_version(&self) -> i64;
    fn capabilities(&self) -> &protocol::ServerCapabilities;

    fn try_next_notification(&self) -> Result<Option<BackendNotification>, BackendError>;
    fn recv_notification(&self) -> Result<BackendNotification, BackendError>;

    fn list_clusters(&self) -> Result<protocol::ListClustersResult, BackendError>;
    fn get_cluster_summary(
        &self,
        params: protocol::GetClusterSummaryParams,
    ) -> Result<protocol::GetClusterSummaryResult, BackendError>;
    fn subscribe_cluster_logs(
        &self,
        params: protocol::SubscribeClusterLogsParams,
    ) -> Result<protocol::SubscribeResult, BackendError>;
    fn subscribe_cluster_timeline(
        &self,
        params: protocol::SubscribeClusterTimelineParams,
    ) -> Result<protocol::SubscribeResult, BackendError>;
    fn start_cluster_from_text(
        &self,
        params: protocol::StartClusterFromTextParams,
    ) -> Result<protocol::StartClusterResult, BackendError>;
    fn start_cluster_from_issue(
        &self,
        params: protocol::StartClusterFromIssueParams,
    ) -> Result<protocol::StartClusterResult, BackendError>;
    fn send_guidance_to_agent(
        &self,
        params: protocol::SendGuidanceToAgentParams,
    ) -> Result<protocol::SendGuidanceToAgentResult, BackendError>;
    fn send_guidance_to_cluster(
        &self,
        params: protocol::SendGuidanceToClusterParams,
    ) -> Result<protocol::SendGuidanceToClusterResult, BackendError>;

    fn shutdown(&self) -> Result<(), BackendError>;
}

pub mod framing;
pub mod stdio;

pub use framing::{encode_frame, FrameDecoder};
