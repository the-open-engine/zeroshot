type ClusterTopologyDeps = {
  getOrchestrator?: () => Promise<any>;
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

let orchestratorPromise: Promise<any> | null = null;

async function getOrchestrator() {
  if (!orchestratorPromise) {
    const Orchestrator = require('../../../src/orchestrator');
    orchestratorPromise = Orchestrator.create({ quiet: true });
  }
  return orchestratorPromise;
}

function normalizeTopic(value: any): string | null {
  if (typeof value !== 'string') {
    return null;
  }
  const topic = value.trim();
  return topic ? topic : null;
}

function extractTopicsFromScript(script: any): string[] {
  if (typeof script !== 'string') {
    return [];
  }
  const topics = new Set<string>();
  const regex = /topic\s*:\s*['"`]([A-Za-z0-9_:-]+)['"`]/g;
  let match: RegExpExecArray | null = null;
  while ((match = regex.exec(script)) !== null) {
    if (match[1]) {
      topics.add(match[1]);
    }
  }
  return Array.from(topics);
}

export function buildTopologyModel(config: any): ClusterTopology {
  const agents: TopologyAgent[] = [];
  const edges: TopologyEdge[] = [];
  const topics = new Set<string>();
  const edgeKeys = new Set<string>();

  const addEdge = (
    from: string,
    to: string,
    topic: string,
    kind: TopologyEdge['kind'],
    dynamic?: boolean
  ) => {
    if (!from || !to || !topic) {
      return;
    }
    const key = `${from}::${to}`;
    if (edgeKeys.has(key)) {
      return;
    }
    edgeKeys.add(key);
    edges.push({ from, to, topic, kind, dynamic });
  };

  topics.add('ISSUE_OPENED');
  addEdge('system', 'ISSUE_OPENED', 'ISSUE_OPENED', 'source');

  const agentConfigs = Array.isArray(config?.agents) ? config.agents : [];
  for (const agent of agentConfigs) {
    const id = typeof agent?.id === 'string' ? agent.id : null;
    if (!id) {
      continue;
    }
    agents.push({
      id,
      role: typeof agent.role === 'string' ? agent.role : null,
    });

    const triggers = Array.isArray(agent.triggers) ? agent.triggers : [];
    for (const trigger of triggers) {
      const topic = normalizeTopic(trigger?.topic);
      if (!topic) {
        continue;
      }
      topics.add(topic);
      addEdge(topic, id, topic, 'trigger');
    }

    const outputTopic = normalizeTopic(agent?.hooks?.onComplete?.config?.topic);
    if (outputTopic) {
      topics.add(outputTopic);
      addEdge(id, outputTopic, outputTopic, 'publish');
    }

    const hookLogicScript = agent?.hooks?.onComplete?.logic?.script;
    for (const topic of extractTopicsFromScript(hookLogicScript)) {
      topics.add(topic);
      addEdge(id, topic, topic, 'publish', true);
    }

    const hookTransformScript = agent?.hooks?.onComplete?.transform?.script;
    for (const topic of extractTopicsFromScript(hookTransformScript)) {
      topics.add(topic);
      addEdge(id, topic, topic, 'publish', true);
    }
  }

  return {
    agents,
    edges,
    topics: Array.from(topics),
  };
}

export async function getClusterTopology(
  clusterId: string | null | undefined,
  { deps = {} }: { deps?: ClusterTopologyDeps } = {}
): Promise<ClusterTopology> {
  if (!clusterId) {
    return { agents: [], edges: [], topics: [] };
  }
  const getOrchestratorImpl = deps.getOrchestrator ?? getOrchestrator;
  const orchestrator = await getOrchestratorImpl();
  const cluster = orchestrator.getCluster(clusterId);
  if (!cluster?.config) {
    throw new Error(`Cluster ${clusterId} not found.`);
  }
  return buildTopologyModel(cluster.config);
}
