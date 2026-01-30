const backend = require("../../../lib/tui-backend/services/cluster-timeline");

export const MAX_TIMELINE_EVENTS: number = backend.MAX_TIMELINE_EVENTS;
export const TIMELINE_POLL_INTERVAL_MS: number = backend.TIMELINE_POLL_INTERVAL_MS;
export const WORKFLOW_TRIGGERS: readonly string[] = backend.WORKFLOW_TRIGGERS;

type ClusterTimelineState = "idle" | "waiting" | "ready" | "error";

export type ClusterTimelineStatus = {
  state: ClusterTimelineState;
  message?: string;
};

export type TimelineEvent = {
  id: string;
  timestamp: number;
  topic: string;
  label: string;
  approved: boolean | null;
  sender: string | null;
};

type ClusterTimelineStreamOptions = {
  clusterId?: string | null;
  onEvents: (events: TimelineEvent[]) => void;
  onStatus?: (status: ClusterTimelineStatus) => void;
  pollIntervalMs?: number;
  maxInitialEvents?: number;
};

export const normalizeTimelineMessage: (message: any) => TimelineEvent | null =
  backend.normalizeTimelineMessage;

export const createClusterTimelineStream: (
  options: ClusterTimelineStreamOptions
) => {
  start: () => void;
  stop: () => void;
  close: () => void;
} = backend.createClusterTimelineStream;
