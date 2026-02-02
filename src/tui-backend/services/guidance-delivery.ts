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

let orchestratorPromise: Promise<any> | null = null;

async function getOrchestrator() {
  if (!orchestratorPromise) {
    const Orchestrator = require('../../../src/orchestrator');
    orchestratorPromise = Orchestrator.create({ quiet: true });
  }
  return orchestratorPromise;
}

type SendAgentGuidanceArgs = {
  clusterId: string;
  agentId: string;
  text: string;
  timeoutMs?: number;
  deps?: GuidanceDeliveryDeps;
};

export async function sendAgentGuidance({
  clusterId,
  agentId,
  text,
  timeoutMs,
  deps = {},
}: SendAgentGuidanceArgs): Promise<GuidanceDeliveryResult> {
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const orchestrator = await getOrchestratorImpl();
  return await orchestrator.sendGuidanceToAgent(clusterId, agentId, text, {
    timeoutMs,
  });
}

type SendClusterGuidanceArgs = {
  clusterId: string;
  text: string;
  timeoutMs?: number;
  deps?: GuidanceDeliveryDeps;
};

export async function sendClusterGuidance({
  clusterId,
  text,
  timeoutMs,
  deps = {},
}: SendClusterGuidanceArgs): Promise<ClusterGuidanceDelivery> {
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const orchestrator = await getOrchestratorImpl();
  return await orchestrator.sendGuidanceToCluster(clusterId, text, {
    timeoutMs,
  });
}
