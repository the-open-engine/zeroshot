const backend = require("../../../lib/tui-backend/services/guidance-delivery");

type GuidanceDeliveryResult = {
  status: string;
  reason: string | null;
  method: string | null;
  taskId?: string | null;
};

type ClusterGuidanceSummary = {
  injected: number;
  queued: number;
  total: number;
};

type ClusterGuidanceDelivery = {
  summary: ClusterGuidanceSummary;
  agents: Record<string, GuidanceDeliveryResult>;
  timestamp: number;
};

type GuidanceDeliveryDeps = {
  getOrchestrator?: () => Promise<any>;
};

type SendAgentGuidanceArgs = {
  clusterId: string;
  agentId: string;
  text: string;
  timeoutMs?: number;
  deps?: GuidanceDeliveryDeps;
};

type SendClusterGuidanceArgs = {
  clusterId: string;
  text: string;
  timeoutMs?: number;
  deps?: GuidanceDeliveryDeps;
};

export const sendAgentGuidance: (
  args: SendAgentGuidanceArgs
) => Promise<GuidanceDeliveryResult> = backend.sendAgentGuidance;

export const sendClusterGuidance: (
  args: SendClusterGuidanceArgs
) => Promise<ClusterGuidanceDelivery> = backend.sendClusterGuidance;
