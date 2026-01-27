type ClusterRegistryDeps = {
  getOrchestrator?: () => Promise<any>;
};

export type ClusterSummary = {
  id: string;
  state: string;
  createdAt: number;
  agentCount: number;
  messageCount: number;
  cwd: string | null;
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

