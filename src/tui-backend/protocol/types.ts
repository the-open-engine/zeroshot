export type JsonRpcId = string | number;

export type RpcErrorData = {
  detail?: string;
  fields?: Record<string, string>;
  supportedVersions?: number[];
};

export type RpcError = {
  code: number;
  message: string;
  data?: RpcErrorData;
};

export type JsonRpcRequest<TParams = unknown> = {
  jsonrpc: '2.0';
  id: JsonRpcId;
  method: string;
  params?: TParams | null;
};

export type JsonRpcNotification<TParams = unknown> = {
  jsonrpc: '2.0';
  method: string;
  params?: TParams | null;
};

export type JsonRpcSuccessResponse<TResult = unknown> = {
  jsonrpc: '2.0';
  id: JsonRpcId;
  result: TResult;
};

export type JsonRpcErrorResponse = {
  jsonrpc: '2.0';
  id: JsonRpcId;
  error: RpcError;
};

export type ClusterSummary = {
  id: string;
  state: string;
  provider: string | null;
  createdAt: number;
  agentCount: number;
  messageCount: number;
  cwd: string | null;
};

export type ClusterMetrics = {
  id: string;
  supported: boolean;
  cpuPercent: number | null;
  memoryMB: number | null;
};

export type ClusterLogLine = {
  id: string;
  timestamp: number;
  text: string;
  agent: string | null;
  role: string | null;
  sender: string | null;
};

export type TimelineEvent = {
  id: string;
  timestamp: number;
  topic: string;
  label: string;
  approved: boolean | null;
  sender: string | null;
};

export type TopologyAgent = {
  id: string;
  role: string | null;
};

export type TopologyEdge = {
  from: string;
  to: string;
  topic: string;
  kind: 'trigger' | 'publish' | 'source';
  dynamic?: boolean;
};

export type ClusterTopology = {
  agents: TopologyAgent[];
  edges: TopologyEdge[];
  topics: string[];
};

export type GuidanceDeliveryResult = {
  status: string;
  reason: string | null;
  method: string | null;
  taskId?: string | null;
};

export type ClusterGuidanceSummary = {
  injected: number;
  queued: number;
  total: number;
};

export type ClusterGuidanceDelivery = {
  summary: ClusterGuidanceSummary;
  agents: Record<string, GuidanceDeliveryResult>;
  timestamp: number;
};

export type InitializeParams = {
  protocolVersion: number;
  client: { name: string; version: string; pid?: number };
  capabilities?: { wantsMetrics?: boolean; wantsTopology?: boolean };
};

export type InitializeResult = {
  protocolVersion: number;
  server: { name: string; version: string };
  capabilities: { methods: string[]; notifications: string[] };
};

export type PingParams = Record<string, never> | null;
export type PingResult = { ok: true };

export type ListClustersResult = { clusters: ClusterSummary[] };
export type GetClusterSummaryParams = { clusterId: string };
export type GetClusterSummaryResult = { summary: ClusterSummary };
export type ListClusterMetricsParams = { clusterIds?: string[] };
export type ListClusterMetricsResult = { metrics: ClusterMetrics[] };
export type StartClusterFromTextParams = {
  text: string;
  providerOverride?: string | null;
  clusterId?: string;
};
export type StartClusterFromIssueParams = {
  ref: string;
  providerOverride?: string | null;
  clusterId?: string;
};
export type StartClusterResult = { clusterId: string };
export type SendGuidanceToAgentParams = {
  clusterId: string;
  agentId: string;
  text: string;
  timeoutMs?: number;
};
export type SendGuidanceToClusterParams = {
  clusterId: string;
  text: string;
  timeoutMs?: number;
};
export type SendGuidanceToAgentResult = { result: GuidanceDeliveryResult };
export type SendGuidanceToClusterResult = { result: ClusterGuidanceDelivery };
export type SubscribeClusterLogsParams = { clusterId: string; agentId?: string | null };
export type SubscribeClusterTimelineParams = { clusterId: string };
export type SubscribeResult = { subscriptionId: string };
export type UnsubscribeParams = { subscriptionId: string };
export type UnsubscribeResult = { removed: boolean };
export type GetClusterTopologyParams = { clusterId: string };
export type GetClusterTopologyResult = { topology: ClusterTopology };

export type ClusterLogLinesNotification = {
  subscriptionId: string;
  clusterId: string;
  lines: ClusterLogLine[];
  droppedCount?: number;
};

export type ClusterTimelineEventsNotification = {
  subscriptionId: string;
  clusterId: string;
  events: TimelineEvent[];
  droppedCount?: number;
};
