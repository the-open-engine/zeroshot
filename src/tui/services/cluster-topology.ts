const backend = require("../../../lib/tui-backend/services/cluster-topology");

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
  kind: "trigger" | "publish" | "source";
  dynamic?: boolean;
};

export type ClusterTopology = {
  agents: TopologyAgent[];
  edges: TopologyEdge[];
  topics: string[];
};

export const buildTopologyModel: (config: any) => ClusterTopology =
  backend.buildTopologyModel;

export const getClusterTopology: (
  clusterId: string | null | undefined,
  options?: { deps?: ClusterTopologyDeps }
) => Promise<ClusterTopology> = backend.getClusterTopology;
