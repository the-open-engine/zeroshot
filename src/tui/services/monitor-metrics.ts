type ClusterSummary = {
  id: string;
  state: string;
  createdAt: number;
  agentCount: number;
  messageCount: number;
  cwd?: string | null;
};

type AgentState = {
  pid?: number | null;
};

type ClusterStatus = {
  agents: AgentState[];
};

type PidusageStat = {
  cpu?: number;
  memory?: number;
};

type PidusageResult = Record<string, PidusageStat> | PidusageStat;
type PidusageFn = (pids: number[] | number) => Promise<PidusageResult>;

type MonitorMetricsDeps = {
  getOrchestrator?: () => Promise<any>;
  pidusage?: PidusageFn;
  platform?: string;
};

export type MonitorClusterRow = ClusterSummary & {
  cpu: number | null;
  memory: number | null;
};

export type MonitorMetricsResult = {
  rows: MonitorClusterRow[];
  error: string | null;
};

const SUPPORTED_PLATFORMS = new Set(["darwin", "linux"]);
let orchestratorPromise: Promise<any> | null = null;

async function getOrchestrator() {
  if (!orchestratorPromise) {
    const Orchestrator = require("../../../src/orchestrator");
    orchestratorPromise = Orchestrator.create({ quiet: true });
  }
  return orchestratorPromise;
}

function extractAgentPids(agents: AgentState[] | null | undefined): number[] {
  if (!agents || agents.length === 0) return [];
  const pids = new Set<number>();
  for (const agent of agents) {
    const pid = agent?.pid;
    if (typeof pid === "number" && Number.isFinite(pid) && pid > 0) {
      pids.add(pid);
    }
  }
  return Array.from(pids);
}

function normalizePidusageResult(
  result: PidusageResult | null | undefined,
  pids: number[]
): Map<number, PidusageStat> {
  const map = new Map<number, PidusageStat>();
  if (!result || pids.length === 0) return map;

  const isSingle =
    typeof (result as PidusageStat).cpu === "number" ||
    typeof (result as PidusageStat).memory === "number";

  if (isSingle && pids.length === 1) {
    map.set(pids[0], result as PidusageStat);
    return map;
  }

  const record = result as Record<string, PidusageStat>;
  for (const pid of pids) {
    const stat = record[String(pid)];
    if (stat) {
      map.set(pid, stat);
    }
  }

  return map;
}

function aggregateStats(stats: Iterable<PidusageStat>): {
  cpu: number | null;
  memory: number | null;
} {
  let cpuTotal = 0;
  let memTotal = 0;
  let hasCpu = false;
  let hasMem = false;

  for (const stat of stats) {
    if (typeof stat.cpu === "number" && Number.isFinite(stat.cpu)) {
      cpuTotal += stat.cpu;
      hasCpu = true;
    }
    if (typeof stat.memory === "number" && Number.isFinite(stat.memory)) {
      memTotal += stat.memory;
      hasMem = true;
    }
  }

  return {
    cpu: hasCpu ? cpuTotal : null,
    memory: hasMem ? memTotal : null,
  };
}

function buildRows(clusters: ClusterSummary[]): MonitorClusterRow[] {
  return clusters.map((cluster) => ({
    ...cluster,
    cpu: null,
    memory: null,
  }));
}

type MetricsErrorState = {
  message: string | null;
};

function getPlatformError(platform: string): string | null {
  if (SUPPORTED_PLATFORMS.has(platform)) return null;
  return `Metrics unsupported on ${platform}.`;
}

async function loadPidusageImpl(
  pidusage?: PidusageFn
): Promise<{ pidusage: PidusageFn | null; error: string | null }> {
  if (pidusage) {
    return { pidusage, error: null };
  }
  try {
    const loaded = require("pidusage") as PidusageFn;
    return { pidusage: loaded, error: null };
  } catch (error) {
    const message =
      error instanceof Error ? error.message : "Unable to load pidusage.";
    return { pidusage: null, error: `Metrics unavailable: ${message}` };
  }
}

function buildRowIndex(rows: MonitorClusterRow[]): Map<string, MonitorClusterRow> {
  return new Map(rows.map((row) => [row.id, row]));
}

function recordMetricsError(
  state: MetricsErrorState,
  error: unknown,
  fallback: string
) {
  if (state.message) return;
  const message = error instanceof Error ? error.message : fallback;
  state.message = `Metrics unavailable: ${message}`;
}

async function updateClusterRow(params: {
  cluster: ClusterSummary;
  rowById: Map<string, MonitorClusterRow>;
  orchestrator: any;
  pidusage: PidusageFn;
  errorState: MetricsErrorState;
}) {
  const { cluster, rowById, orchestrator, pidusage, errorState } = params;
  const row = rowById.get(cluster.id);
  if (!row) return;

  let status: ClusterStatus | null = null;
  try {
    status = orchestrator.getStatus(cluster.id) as ClusterStatus;
  } catch (error) {
    recordMetricsError(errorState, error, "Status error.");
    return;
  }

  const pids = extractAgentPids(status?.agents);
  if (pids.length === 0) return;

  try {
    const raw = await pidusage(pids);
    const normalized = normalizePidusageResult(raw, pids);
    const aggregate = aggregateStats(normalized.values());
    row.cpu = aggregate.cpu;
    row.memory = aggregate.memory;
  } catch (error) {
    recordMetricsError(errorState, error, "pidusage error.");
  }
}

async function applyClusterMetrics(params: {
  clusters: ClusterSummary[];
  rows: MonitorClusterRow[];
  orchestrator: any;
  pidusage: PidusageFn;
  errorState: MetricsErrorState;
}) {
  const { clusters, rows, orchestrator, pidusage, errorState } = params;
  const rowById = buildRowIndex(rows);
  const runningClusters = clusters.filter((cluster) => cluster.state === "running");
  await Promise.all(
    runningClusters.map((cluster) =>
      updateClusterRow({ cluster, rowById, orchestrator, pidusage, errorState })
    )
  );
}

export async function fetchMonitorMetrics(
  deps: MonitorMetricsDeps & { clusters?: ClusterSummary[] } = {}
): Promise<MonitorMetricsResult> {
  const platform = deps.platform ?? process.platform;
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const orchestrator = await getOrchestratorImpl();
  const clusters =
    deps.clusters ?? (orchestrator.listClusters() as ClusterSummary[]);
  const rows = buildRows(clusters);
  const platformError = getPlatformError(platform);
  if (platformError) return { rows, error: platformError };

  const { pidusage, error } = await loadPidusageImpl(deps.pidusage);
  if (!pidusage || error) return { rows, error };

  const errorState: MetricsErrorState = { message: null };
  await applyClusterMetrics({
    clusters,
    rows,
    orchestrator,
    pidusage,
    errorState,
  });

  return { rows, error: errorState.message };
}
