const pidusage = require("pidusage");

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
    const Orchestrator = require("../../../src/orchestrator");
    orchestratorPromise = Orchestrator.create({ quiet: true });
  }
  return orchestratorPromise;
}

function resolveClusterCwd(cluster: any): string | null {
  if (!cluster || typeof cluster !== "object") {
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

function resolveAgentPid(agent: any): number | null {
  if (!agent || typeof agent !== "object") {
    return null;
  }
  const pid = agent.processPid ?? agent.pid ?? null;
  if (Number.isFinite(pid) && pid > 0) {
    return pid;
  }
  if (typeof agent.getState === "function") {
    const state = agent.getState();
    const statePid = state?.pid ?? null;
    if (Number.isFinite(statePid) && statePid > 0) {
      return statePid;
    }
  }
  return null;
}

function collectAgentPids(cluster: any): number[] {
  if (!cluster || typeof cluster !== "object") {
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

function normalizeSummary(summary: any, orchestrator: any): ClusterSummary {
  if (!summary || typeof summary !== "object") {
    throw new Error("Invalid cluster summary.");
  }
  if (typeof summary.id !== "string" || summary.id.length === 0) {
    throw new Error("Invalid cluster id.");
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
  return {
    id: summary.id,
    state: String(summary.state ?? "unknown"),
    createdAt: summary.createdAt,
    agentCount: summary.agentCount,
    messageCount: summary.messageCount,
    cwd,
  };
}

type ListClustersArgs = {
  deps?: ClusterRegistryDeps;
};

type ListClusterMetricsArgs = {
  deps?: ClusterRegistryDeps;
};

const SUPPORTED_PLATFORMS = new Set(["darwin", "linux"]);
const BYTES_PER_MB = 1024 * 1024;

export async function listClusters(
  { deps = {} }: ListClustersArgs = {}
): Promise<ClusterSummary[]> {
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const orchestrator = await getOrchestratorImpl();
  const summaries = orchestrator.listClusters();
  const results = summaries.map((summary: any) =>
    normalizeSummary(summary, orchestrator)
  );
  results.sort((left, right) => {
    if (left.createdAt !== right.createdAt) {
      return left.createdAt - right.createdAt;
    }
    return left.id.localeCompare(right.id);
  });
  return results;
}

export async function listClusterMetrics(
  { deps = {} }: ListClusterMetricsArgs = {}
): Promise<Record<string, ClusterMetrics>> {
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const pidusageImpl = deps.pidusage ?? pidusage;
  const platform = deps.platform ?? process.platform;
  const orchestrator = await getOrchestratorImpl();
  const summaries = orchestrator.listClusters();
  const clusterIds = summaries.map((summary: any) => summary.id);

  if (!SUPPORTED_PLATFORMS.has(platform)) {
    return Object.fromEntries(
      clusterIds.map((id) => [
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
  for (const clusterId of clusterIds) {
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
  for (const clusterId of clusterIds) {
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
