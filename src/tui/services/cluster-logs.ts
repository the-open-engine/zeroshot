const backend = require("../../../lib/tui-backend/services/cluster-logs");

export const MAX_LOG_LINES: number = backend.MAX_LOG_LINES;
export const LOG_POLL_INTERVAL_MS: number = backend.LOG_POLL_INTERVAL_MS;

type ClusterLogState = "idle" | "waiting" | "ready" | "error";

export type ClusterLogStatus = {
  state: ClusterLogState;
  message?: string;
};

export type ClusterLogLine = {
  id: string;
  timestamp: number;
  text: string;
  agent: string | null;
  role: string | null;
  sender: string | null;
};

type ClusterLogStreamOptions = {
  clusterId?: string | null;
  agentId?: string | null;
  onLines: (lines: ClusterLogLine[]) => void;
  onStatus?: (status: ClusterLogStatus) => void;
  pollIntervalMs?: number;
  maxInitialLines?: number;
};

export const resolveClusterDbPath: (clusterId: string) => string =
  backend.resolveClusterDbPath;

export const normalizeAgentOutput: (message: any) => ClusterLogLine | null =
  backend.normalizeAgentOutput;

export const createClusterLogStream: (
  options: ClusterLogStreamOptions
) => {
  start: () => void;
  stop: () => void;
  close: () => void;
} = backend.createClusterLogStream;
