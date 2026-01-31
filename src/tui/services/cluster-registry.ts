const backend = require("../../../lib/tui-backend/services/cluster-registry");

type PidusageStats = Record<string, { cpu?: number; memory?: number }>;
type PidusageFn = (pids: number[]) => Promise<PidusageStats>;

type ClusterRegistryDeps = {
  getOrchestrator?: () => Promise<any>;
  pidusage?: PidusageFn;
  platform?: string;
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

type ListClustersArgs = {
  deps?: ClusterRegistryDeps;
};

type ListClusterMetricsArgs = {
  deps?: ClusterRegistryDeps;
};

export const listClusters: (
  args?: ListClustersArgs
) => Promise<ClusterSummary[]> = backend.listClusters;

export const listClusterMetrics: (
  args?: ListClusterMetricsArgs
) => Promise<Record<string, ClusterMetrics>> = backend.listClusterMetrics;
