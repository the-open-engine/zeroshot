use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    String(String),
    Number(i64),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest<T> {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<T>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification<T> {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<T>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcSuccessResponse<T> {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub result: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub error: RpcError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcErrorData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<RpcErrorData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterSummary {
    pub id: String,
    pub state: String,
    pub provider: Option<String>,
    pub created_at: i64,
    pub agent_count: i64,
    pub message_count: i64,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterMetrics {
    pub id: String,
    pub supported: bool,
    pub cpu_percent: Option<f64>,
    pub memory_mb: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterLogLine {
    pub id: String,
    pub timestamp: i64,
    pub text: String,
    pub agent: Option<String>,
    pub role: Option<String>,
    pub sender: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEvent {
    pub id: String,
    pub timestamp: i64,
    pub topic: String,
    pub label: String,
    pub approved: Option<bool>,
    pub sender: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyAgent {
    pub id: String,
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyEdge {
    pub from: String,
    pub to: String,
    pub topic: String,
    pub kind: TopologyEdgeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TopologyEdgeKind {
    Trigger,
    Publish,
    Source,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterTopology {
    pub agents: Vec<TopologyAgent>,
    pub edges: Vec<TopologyEdge>,
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuidanceDeliveryResult {
    pub status: String,
    pub reason: Option<String>,
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterGuidanceSummary {
    pub injected: i64,
    pub queued: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterGuidanceDelivery {
    pub summary: ClusterGuidanceSummary,
    pub agents: HashMap<String, GuidanceDeliveryResult>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wants_metrics: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wants_topology: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: i64,
    pub client: ClientInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ClientCapabilities>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    pub methods: Vec<String>,
    pub notifications: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: i64,
    pub server: ServerInfo,
    pub capabilities: ServerCapabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListClustersResult {
    pub clusters: Vec<ClusterSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetClusterSummaryParams {
    pub cluster_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetClusterSummaryResult {
    pub summary: ClusterSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListClusterMetricsParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListClusterMetricsResult {
    pub metrics: Vec<ClusterMetrics>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartClusterFromTextParams {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartClusterFromIssueParams {
    #[serde(rename = "ref")]
    pub r#ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_override: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartClusterResult {
    pub cluster_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendGuidanceToAgentParams {
    pub cluster_id: String,
    pub agent_id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendGuidanceToClusterParams {
    pub cluster_id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendGuidanceToAgentResult {
    pub result: GuidanceDeliveryResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendGuidanceToClusterResult {
    pub result: ClusterGuidanceDelivery,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeClusterLogsParams {
    pub cluster_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeClusterTimelineParams {
    pub cluster_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeResult {
    pub subscription_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeParams {
    pub subscription_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeResult {
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetClusterTopologyParams {
    pub cluster_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetClusterTopologyResult {
    pub topology: ClusterTopology,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterLogLinesParams {
    pub subscription_id: String,
    pub cluster_id: String,
    pub lines: Vec<ClusterLogLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterTimelineEventsParams {
    pub subscription_id: String,
    pub cluster_id: String,
    pub events: Vec<TimelineEvent>,
}

pub type InitializeRequest = JsonRpcRequest<InitializeParams>;
pub type InitializeResponse = JsonRpcSuccessResponse<InitializeResult>;
pub type ListClustersRequest = JsonRpcRequest<ListClustersParams>;
pub type ListClustersResponse = JsonRpcSuccessResponse<ListClustersResult>;
pub type GetClusterSummaryRequest = JsonRpcRequest<GetClusterSummaryParams>;
pub type GetClusterSummaryResponse = JsonRpcSuccessResponse<GetClusterSummaryResult>;
pub type ListClusterMetricsRequest = JsonRpcRequest<ListClusterMetricsParams>;
pub type ListClusterMetricsResponse = JsonRpcSuccessResponse<ListClusterMetricsResult>;
pub type StartClusterFromTextRequest = JsonRpcRequest<StartClusterFromTextParams>;
pub type StartClusterFromTextResponse = JsonRpcSuccessResponse<StartClusterResult>;
pub type StartClusterFromIssueRequest = JsonRpcRequest<StartClusterFromIssueParams>;
pub type StartClusterFromIssueResponse = JsonRpcSuccessResponse<StartClusterResult>;
pub type SendGuidanceToAgentRequest = JsonRpcRequest<SendGuidanceToAgentParams>;
pub type SendGuidanceToAgentResponse = JsonRpcSuccessResponse<SendGuidanceToAgentResult>;
pub type SendGuidanceToClusterRequest = JsonRpcRequest<SendGuidanceToClusterParams>;
pub type SendGuidanceToClusterResponse = JsonRpcSuccessResponse<SendGuidanceToClusterResult>;
pub type SubscribeClusterLogsRequest = JsonRpcRequest<SubscribeClusterLogsParams>;
pub type SubscribeClusterLogsResponse = JsonRpcSuccessResponse<SubscribeResult>;
pub type SubscribeClusterTimelineRequest = JsonRpcRequest<SubscribeClusterTimelineParams>;
pub type SubscribeClusterTimelineResponse = JsonRpcSuccessResponse<SubscribeResult>;
pub type UnsubscribeRequest = JsonRpcRequest<UnsubscribeParams>;
pub type UnsubscribeResponse = JsonRpcSuccessResponse<UnsubscribeResult>;
pub type GetClusterTopologyRequest = JsonRpcRequest<GetClusterTopologyParams>;
pub type GetClusterTopologyResponse = JsonRpcSuccessResponse<GetClusterTopologyResult>;

pub type ClusterLogLinesNotification = JsonRpcNotification<ClusterLogLinesParams>;
pub type ClusterTimelineEventsNotification = JsonRpcNotification<ClusterTimelineEventsParams>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListClustersParams {}
