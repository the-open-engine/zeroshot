import React, { useEffect, useMemo, useRef, useState } from "react";
import { Box, Text, useInput } from "ink";
import {
  ClusterLogLine,
  ClusterLogStatus,
  createClusterLogStream,
  MAX_LOG_LINES,
} from "../services/cluster-logs";
import {
  ClusterTimelineStatus,
  createClusterTimelineStream,
  MAX_TIMELINE_EVENTS,
  TimelineEvent,
} from "../services/cluster-timeline";
import {
  ClusterTopology,
  getClusterTopology,
  TopologyEdge,
} from "../services/cluster-topology";

type ClusterViewProps = {
  provider: string | null;
  clusterId?: string | null;
  selectedAgentId?: string | null;
  onSelectAgent?: (agentId: string | null) => void;
  onOpenAgent?: (agentId: string) => void;
  isCommandInputEmpty?: boolean;
  guidanceHistory?: Array<{
    text: string;
    timestamp: number;
    injectedCount: number;
    queuedCount: number;
  }>;
};

type ClusterLogStreamHandle = ReturnType<typeof createClusterLogStream>;
type ClusterTimelineStreamHandle = ReturnType<typeof createClusterTimelineStream>;

const EMPTY_STATUS: ClusterLogStatus = { state: "idle" };
const EMPTY_TIMELINE_STATUS: ClusterTimelineStatus = { state: "idle" };
const TOPOLOGY_REFRESH_INTERVAL_MS = 1500;

function padTime(value: number): string {
  return value.toString().padStart(2, "0");
}

function formatTimestamp(timestamp: number): string {
  const date = new Date(timestamp);
  return `${padTime(date.getHours())}:${padTime(date.getMinutes())}:${padTime(
    date.getSeconds()
  )}`;
}

function formatLogLine(line: ClusterLogLine): string {
  const agent = line.agent || line.sender || "agent";
  const roleSuffix = line.role ? `/${line.role}` : "";
  return `[${formatTimestamp(line.timestamp)}] ${agent}${roleSuffix}: ${line.text}`;
}

function formatTimelineEvent(event: TimelineEvent): string {
  return `[${formatTimestamp(event.timestamp)}] ${event.label}`;
}

export default function ClusterView({
  provider,
  clusterId,
  selectedAgentId,
  onSelectAgent,
  onOpenAgent,
  isCommandInputEmpty = false,
  guidanceHistory = [],
}: ClusterViewProps) {
  const [logLines, setLogLines] = useState<ClusterLogLine[]>([]);
  const [logStatus, setLogStatus] = useState<ClusterLogStatus>(EMPTY_STATUS);
  const streamRef = useRef<ClusterLogStreamHandle | null>(null);
  const [timelineEvents, setTimelineEvents] = useState<TimelineEvent[]>([]);
  const [timelineStatus, setTimelineStatus] =
    useState<ClusterTimelineStatus>(EMPTY_TIMELINE_STATUS);
  const timelineStreamRef = useRef<ClusterTimelineStreamHandle | null>(null);
  const [topology, setTopology] = useState<ClusterTopology | null>(null);
  const [topologyError, setTopologyError] = useState<string | null>(null);
  const [topologyLoading, setTopologyLoading] = useState(false);
  const agents = topology?.agents ?? [];

  useEffect(() => {
    if (!onSelectAgent) {
      return;
    }
    if (!agents.length) {
      if (selectedAgentId) {
        onSelectAgent(null);
      }
      return;
    }
    const hasSelection = Boolean(
      selectedAgentId && agents.some((agent) => agent.id === selectedAgentId)
    );
    if (!hasSelection) {
      onSelectAgent(agents[0].id);
    }
  }, [agents, onSelectAgent, selectedAgentId]);

  const selectedIndex = useMemo(() => {
    if (agents.length === 0) {
      return -1;
    }
    const index = agents.findIndex((agent) => agent.id === selectedAgentId);
    return index >= 0 ? index : 0;
  }, [agents, selectedAgentId]);

  useEffect(() => {
    setLogLines([]);
    setLogStatus(clusterId ? { state: "waiting" } : EMPTY_STATUS);

    if (streamRef.current) {
      streamRef.current.close();
      streamRef.current = null;
    }

    const stream = createClusterLogStream({
      clusterId,
      onLines: (lines) => {
        setLogLines((prev) => {
          const next = prev.concat(lines);
          if (next.length <= MAX_LOG_LINES) {
            return next;
          }
          return next.slice(next.length - MAX_LOG_LINES);
        });
      },
      onStatus: setLogStatus,
    });

    stream.start();
    streamRef.current = stream;

    return () => {
      stream.close();
      streamRef.current = null;
    };
  }, [clusterId]);

  useEffect(() => {
    setTimelineEvents([]);
    setTimelineStatus(clusterId ? { state: "waiting" } : EMPTY_TIMELINE_STATUS);

    if (timelineStreamRef.current) {
      timelineStreamRef.current.close();
      timelineStreamRef.current = null;
    }

    const stream = createClusterTimelineStream({
      clusterId,
      onEvents: (events) => {
        setTimelineEvents((prev) => {
          const merged = new Map<string, TimelineEvent>();
          for (const event of prev) {
            merged.set(event.id, event);
          }
          for (const event of events) {
            merged.set(event.id, event);
          }
          const list = Array.from(merged.values()).sort(
            (a, b) => a.timestamp - b.timestamp
          );
          if (list.length <= MAX_TIMELINE_EVENTS) {
            return list;
          }
          return list.slice(list.length - MAX_TIMELINE_EVENTS);
        });
      },
      onStatus: setTimelineStatus,
    });

    stream.start();
    timelineStreamRef.current = stream;

    return () => {
      stream.close();
      timelineStreamRef.current = null;
    };
  }, [clusterId]);

  useEffect(() => {
    let cancelled = false;
    let inFlight = false;

    async function refresh() {
      if (!clusterId) {
        setTopology(null);
        setTopologyError(null);
        setTopologyLoading(false);
        return;
      }
      if (inFlight) {
        return;
      }
      inFlight = true;
      setTopologyLoading(true);
      try {
        const result = await getClusterTopology(clusterId);
        if (cancelled) {
          return;
        }
        setTopology(result);
        setTopologyError(null);
      } catch (err) {
        if (cancelled) {
          return;
        }
        const message = err instanceof Error ? err.message : "Failed to load topology.";
        setTopologyError(message);
        setTopology(null);
      } finally {
        if (!cancelled) {
          setTopologyLoading(false);
        }
        inFlight = false;
      }
    }

    void refresh();
    const interval = setInterval(refresh, TOPOLOGY_REFRESH_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [clusterId]);

  const emptyMessage = useMemo(() => {
    if (!clusterId) {
      return "No cluster selected.";
    }
    if (logStatus.state === "waiting") {
      return "Waiting for cluster logs...";
    }
    if (logStatus.state === "error") {
      return logStatus.message
        ? `Log error: ${logStatus.message}`
        : "Log error.";
    }
    return "No logs yet.";
  }, [clusterId, logStatus]);

  const timelineMessage = useMemo(() => {
    if (!clusterId) {
      return "No cluster selected.";
    }
    if (timelineStatus.state === "waiting") {
      return "Waiting for timeline...";
    }
    if (timelineStatus.state === "error") {
      return timelineStatus.message
        ? `Timeline error: ${timelineStatus.message}`
        : "Timeline error.";
    }
    return "No timeline events yet.";
  }, [clusterId, timelineStatus]);

  const topologyLines = useMemo(() => {
    if (!topology) {
      return [] as string[];
    }
    const adjacency = new Map<string, string[]>();

    const addEdge = (edge: TopologyEdge) => {
      const list = adjacency.get(edge.from) ?? [];
      if (!list.includes(edge.to)) {
        list.push(edge.to);
      }
      adjacency.set(edge.from, list);
    };

    for (const edge of topology.edges) {
      addEdge(edge);
    }

    return Array.from(adjacency.entries()).map(
      ([from, tos]) => `${from} -> ${tos.join(", ")}`
    );
  }, [topology]);

  const topologyMessage = useMemo(() => {
    if (!clusterId) {
      return "No cluster selected.";
    }
    if (topologyError) {
      return `Topology error: ${topologyError}`;
    }
    if (topologyLoading && !topology) {
      return "Loading topology...";
    }
    if (!topology || topology.agents.length === 0) {
      return "No topology data.";
    }
    return null;
  }, [clusterId, topologyError, topologyLoading, topology]);

  useInput((_input, key) => {
    if (!agents.length) {
      return;
    }
    const currentIndex = selectedIndex >= 0 ? selectedIndex : 0;

    if (key.upArrow) {
      const nextIndex = Math.max(0, currentIndex - 1);
      const nextAgent = agents[nextIndex];
      if (nextAgent && nextAgent.id !== selectedAgentId) {
        onSelectAgent?.(nextAgent.id);
      }
      return;
    }
    if (key.downArrow) {
      const nextIndex = Math.min(agents.length - 1, currentIndex + 1);
      const nextAgent = agents[nextIndex];
      if (nextAgent && nextAgent.id !== selectedAgentId) {
        onSelectAgent?.(nextAgent.id);
      }
      return;
    }
    if (key.return && isCommandInputEmpty) {
      const agent = agents[currentIndex];
      if (agent && onOpenAgent) {
        onOpenAgent(agent.id);
      }
    }
  });

  return (
    <Box flexDirection="column">
      <Box flexDirection="column" marginBottom={1}>
        <Text color="cyan">Cluster</Text>
        <Text color="gray">Id: {clusterId || "pending"}</Text>
        <Text color="gray">Provider: {provider || "auto"}</Text>
      </Box>

      <Box flexDirection="column" marginBottom={1}>
        <Text color="yellow">Topology</Text>
        {topologyMessage ? (
          <Text color="gray">{topologyMessage}</Text>
        ) : (
          <Box flexDirection="column">
            <Text color="gray">Agents</Text>
            {agents.map((agent, index) => {
              const suffix = agent.role ? ` (${agent.role})` : "";
              const isSelected = index === selectedIndex;
              const prefix = isSelected ? "›" : " ";
              return (
                <Text key={agent.id} inverse={isSelected}>
                  {prefix} {agent.id}
                  {suffix}
                </Text>
              );
            })}
            <Text color="gray">Wiring</Text>
            {topologyLines.length === 0 ? (
              <Text color="gray">No wiring detected.</Text>
            ) : (
              topologyLines.map((line) => <Text key={line}>{line}</Text>)
            )}
          </Box>
        )}
      </Box>

      <Box flexDirection="column" marginBottom={1}>
        <Text color="yellow">Timeline</Text>
        {timelineEvents.length === 0 ? (
          <Text color="gray">{timelineMessage}</Text>
        ) : (
          timelineEvents.map((event) => (
            <Text key={event.id}>{formatTimelineEvent(event)}</Text>
          ))
        )}
      </Box>

      {guidanceHistory.length > 0 && (
        <Box flexDirection="column" marginBottom={1}>
          <Text color="yellow">Guidance history</Text>
          {guidanceHistory.map((item, index) => {
            const statusText =
              item.injectedCount > 0
                ? `${item.injectedCount} injected, ${item.queuedCount} queued`
                : `${item.queuedCount} queued`;
            return (
              <Text key={index}>
                [{formatTimestamp(item.timestamp)}] {statusText}: {item.text}
              </Text>
            );
          })}
        </Box>
      )}

      <Box flexDirection="column" flexGrow={1}>
        <Text color="yellow">Live logs</Text>
        {logLines.length === 0 ? (
          <Text color="gray">{emptyMessage}</Text>
        ) : (
          logLines.map((line) => (
            <Text key={line.id}>{formatLogLine(line)}</Text>
          ))
        )}
      </Box>

      <Box flexDirection="column" marginTop={1}>
        <Text color="gray">Type /help for commands</Text>
        <Text color="gray">
          Keys: Up/Down select, Enter open (empty input), Esc back
        </Text>
      </Box>
    </Box>
  );
}
