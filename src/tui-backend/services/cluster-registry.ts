import { loadSettings } from '../../../lib/settings';
import { normalizeProviderName } from '../../../lib/provider-names';

const pidusage = require('pidusage');

type PidusageStats = Record<string, { cpu?: number; memory?: number }>;
type PidusageFn = (pids: number[]) => Promise<PidusageStats>;

type ClusterRegistryDeps = {
  getOrchestrator?: () => Promise<any>;
  pidusage?: PidusageFn;
  platform?: string;
  loadSettings?: typeof loadSettings;
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

let orchestratorPromise: Promise<any> | null = null;

async function getOrchestrator() {
  if (!orchestratorPromise) {
    const Orchestrator = require('../../../src/orchestrator');
    orchestratorPromise = Orchestrator.create({ quiet: true });
  }
  return orchestratorPromise;
}

function resolveClusterCwd(cluster: any): string | null {
  if (!cluster || typeof cluster !== 'object') {
    return null;
  }
  if (cluster.worktree?.path) {
    return cluster.worktree.path;
  }
  if (cluster.isolation?.workDir) {
    return cluster.isolation.workDir;
  }
  return null;
}

function resolveClusterProvider(cluster: any, settings: any): string | null {
  if (!cluster || typeof cluster !== 'object') {
    const fallback = settings?.defaultProvider ?? null;
    const normalizedFallback = normalizeProviderName(fallback);
    return typeof normalizedFallback === 'string' ? normalizedFallback : null;
  }
  const forced = cluster.config?.forceProvider ?? null;
  const defaultProvider = cluster.config?.defaultProvider ?? null;
  const settingsProvider = settings?.defaultProvider ?? null;
  const provider =
    forced && typeof forced === 'string'
      ? forced
      : defaultProvider && typeof defaultProvider === 'string'
        ? defaultProvider
        : settingsProvider && typeof settingsProvider === 'string'
          ? settingsProvider
          : null;
  const normalized = normalizeProviderName(provider);
  return typeof normalized === 'string' ? normalized : null;
}

function resolveAgentPid(agent: any): number | null {
  if (!agent || typeof agent !== 'object') {
    return null;
  }
  const pid = agent.processPid ?? agent.pid ?? null;
  if (Number.isFinite(pid) && pid > 0) {
    return pid;
  }
  if (typeof agent.getState === 'function') {
    const state = agent.getState();
    const statePid = state?.pid ?? null;
    if (Number.isFinite(statePid) && statePid > 0) {
      return statePid;
    }
  }
  return null;
}

function collectAgentPids(cluster: any): number[] {
  if (!cluster || typeof cluster !== 'object') {
    return [];
  }
  const agents = Array.isArray(cluster.agents) ? cluster.agents : [];
  const pids = new Set<number>();
  for (const agent of agents) {
    const pid = resolveAgentPid(agent);
    if (pid) {
      pids.add(pid);
    }
  }
  return Array.from(pids);
}

function normalizeSummary(summary: any, orchestrator: any, settings: any): ClusterSummary {
  if (!summary || typeof summary !== 'object') {
    throw new Error('Invalid cluster summary.');
  }
  if (typeof summary.id !== 'string' || summary.id.length === 0) {
    throw new Error('Invalid cluster id.');
  }
  if (!Number.isFinite(summary.createdAt)) {
    throw new Error(`Invalid createdAt for cluster ${summary.id}.`);
  }
  if (!Number.isFinite(summary.agentCount)) {
    throw new Error(`Invalid agentCount for cluster ${summary.id}.`);
  }
  if (!Number.isFinite(summary.messageCount)) {
    throw new Error(`Invalid messageCount for cluster ${summary.id}.`);
  }
  const cluster = orchestrator.getCluster(summary.id);
  const cwd = resolveClusterCwd(cluster);
  const provider = resolveClusterProvider(cluster, settings);
  return {
    id: summary.id,
    state: String(summary.state ?? 'unknown'),
    provider,
    createdAt: summary.createdAt,
    agentCount: summary.agentCount,
    messageCount: summary.messageCount,
    cwd,
  };
}

export class ClusterNotFoundError extends Error {
  clusterId: string;

  constructor(clusterId: string) {
    super(`Cluster not found: ${clusterId}`);
    this.name = 'ClusterNotFoundError';
    this.clusterId = clusterId;
  }
}

type ListClustersArgs = {
  deps?: ClusterRegistryDeps;
};

type ListClusterMetricsArgs = {
  clusterIds?: string[];
  deps?: ClusterRegistryDeps;
};

type GetClusterSummaryArgs = {
  clusterId: string;
  deps?: ClusterRegistryDeps;
};

const SUPPORTED_PLATFORMS = new Set(['darwin', 'linux']);
const BYTES_PER_MB = 1024 * 1024;

export async function listClusters({ deps = {} }: ListClustersArgs = {}): Promise<
  ClusterSummary[]
> {
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const loadSettingsImpl = deps.loadSettings ?? loadSettings;
  const orchestrator = await getOrchestratorImpl();
  const settings = loadSettingsImpl();
  const summaries = orchestrator.listClusters();
  const results = summaries.map((summary: any) =>
    normalizeSummary(summary, orchestrator, settings)
  );
  results.sort((left, right) => {
    if (left.createdAt !== right.createdAt) {
      return left.createdAt - right.createdAt;
    }
    return left.id.localeCompare(right.id);
  });
  return results;
}

export async function getClusterSummary({
  clusterId,
  deps = {},
}: GetClusterSummaryArgs): Promise<ClusterSummary> {
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const loadSettingsImpl = deps.loadSettings ?? loadSettings;
  const orchestrator = await getOrchestratorImpl();
  const settings = loadSettingsImpl();
  const summaries = orchestrator.listClusters();
  const summary = summaries.find((entry: any) => entry.id === clusterId);
  if (!summary) {
    throw new ClusterNotFoundError(clusterId);
  }
  return normalizeSummary(summary, orchestrator, settings);
}

export async function listClusterMetrics({
  clusterIds,
  deps = {},
}: ListClusterMetricsArgs = {}): Promise<Record<string, ClusterMetrics>> {
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const pidusageImpl = deps.pidusage ?? pidusage;
  const platform = deps.platform ?? process.platform;
  const orchestrator = await getOrchestratorImpl();
  const summaries = orchestrator.listClusters();
  const availableIds = summaries.map((summary: any) => summary.id);
  const requestedIds = Array.isArray(clusterIds)
    ? clusterIds.filter((id) => typeof id === 'string')
    : null;
  let resolvedIds = availableIds;
  if (requestedIds) {
    if (requestedIds.length === 0) {
      return {};
    }
    const availableSet = new Set(availableIds);
    resolvedIds = requestedIds.filter((id) => availableSet.has(id));
  }

  if (resolvedIds.length === 0) {
    return {};
  }

  if (!SUPPORTED_PLATFORMS.has(platform)) {
    return Object.fromEntries(
      resolvedIds.map((id) => [
        id,
        {
          id,
          supported: false,
          cpuPercent: null,
          memoryMB: null,
        },
      ])
    );
  }

  const pidsByCluster = new Map<string, number[]>();
  const allPids = new Set<number>();
  for (const clusterId of resolvedIds) {
    const cluster = orchestrator.getCluster(clusterId);
    const pids = collectAgentPids(cluster);
    pidsByCluster.set(clusterId, pids);
    for (const pid of pids) {
      allPids.add(pid);
    }
  }

  let statsByPid: PidusageStats = {};
  if (allPids.size > 0) {
    try {
      statsByPid = await pidusageImpl(Array.from(allPids));
    } catch {
      statsByPid = {};
    }
  }

  const results: Record<string, ClusterMetrics> = {};
  for (const clusterId of resolvedIds) {
    const pids = pidsByCluster.get(clusterId) ?? [];
    let cpuTotal = 0;
    let memoryTotalBytes = 0;
    let hasCpu = false;
    let hasMemory = false;
    for (const pid of pids) {
      const stats = statsByPid[String(pid)] ?? statsByPid[pid as any];
      if (!stats) {
        continue;
      }
      const cpu = Number(stats.cpu);
      const memory = Number(stats.memory);
      if (Number.isFinite(cpu)) {
        cpuTotal += cpu;
        hasCpu = true;
      }
      if (Number.isFinite(memory)) {
        memoryTotalBytes += memory;
        hasMemory = true;
      }
    }
    results[clusterId] = {
      id: clusterId,
      supported: true,
      cpuPercent: hasCpu ? cpuTotal : null,
      memoryMB: hasMemory ? memoryTotalBytes / BYTES_PER_MB : null,
    };
  }

  return results;
}
