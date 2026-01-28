import React, { useEffect, useMemo, useRef, useState } from "react";
import { Box, Text } from "ink";
import {
  ClusterLogLine,
  ClusterLogStatus,
  createClusterLogStream,
  MAX_LOG_LINES,
} from "../services/cluster-logs";
import { PendingAgentMessage } from "../services/agent-messages";

type AgentViewProps = {
  provider: string | null;
  clusterId?: string | null;
  agentId?: string | null;
  pendingMessages?: PendingAgentMessage[];
  messageDraft?: string;
  inputMode?: "command" | "message";
};

type ClusterLogStreamHandle = ReturnType<typeof createClusterLogStream>;
const EMPTY_STATUS: ClusterLogStatus = { state: "idle" };

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

function formatPendingMessage(message: PendingAgentMessage): string {
  const statusLabel = message.deliveryStatus
    ? message.deliveryStatus.status
    : "pending";
  return `[${formatTimestamp(message.createdAt)}] ${statusLabel}: ${message.text}`;
}

export default function AgentView({
  provider,
  clusterId,
  agentId,
  pendingMessages,
  messageDraft,
  inputMode = "message",
}: AgentViewProps) {
  const [logLines, setLogLines] = useState<ClusterLogLine[]>([]);
  const [logStatus, setLogStatus] = useState<ClusterLogStatus>(EMPTY_STATUS);
  const streamRef = useRef<ClusterLogStreamHandle | null>(null);
  const pending = pendingMessages ?? [];
  const draft = messageDraft ?? "";
  const messagePlaceholder =
    inputMode === "command" ? "Type a command (/help)" : "Type a message";
  const modeLabel =
    inputMode === "command"
      ? "Command mode (starts with /)"
      : "Message mode (Enter queues)";

  useEffect(() => {
    setLogLines([]);
    if (clusterId && agentId) {
      setLogStatus({ state: "waiting" });
    } else {
      setLogStatus(EMPTY_STATUS);
    }

    if (streamRef.current) {
      streamRef.current.close();
      streamRef.current = null;
    }

    if (!clusterId || !agentId) {
      return;
    }

    const stream = createClusterLogStream({
      clusterId,
      agentId,
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
  }, [agentId, clusterId]);

  const emptyMessage = useMemo(() => {
    if (!clusterId) {
      return "No cluster selected.";
    }
    if (!agentId) {
      return "No agent selected.";
    }
    if (logStatus.state === "waiting") {
      return "Waiting for agent logs...";
    }
    if (logStatus.state === "error") {
      return logStatus.message
        ? `Log error: ${logStatus.message}`
        : "Log error.";
    }
    return "No logs yet.";
  }, [agentId, clusterId, logStatus]);

  return (
    <Box flexDirection="column">
      <Text color="cyan">Agent</Text>
      <Text color="gray">Cluster: {clusterId || "pending"}</Text>
      <Text color="gray">Agent: {agentId || "pending"}</Text>
      <Text color="gray">Provider: {provider || "auto"}</Text>

      <Box flexDirection="column" marginTop={1}>
        <Text color="yellow">Message</Text>
        <Box>
          <Text color="cyan">{"> "}</Text>
          {draft ? (
            <Text>{draft}</Text>
          ) : (
            <Text color="gray">{messagePlaceholder}</Text>
          )}
        </Box>
        <Text color="gray">{modeLabel}</Text>
      </Box>

      <Box flexDirection="column" marginTop={1}>
        <Text color="yellow">Guidance messages</Text>
        {pending.length === 0 ? (
          <Text color="gray">No guidance messages.</Text>
        ) : (
          pending.map((message) => (
            <Text key={message.id}>{formatPendingMessage(message)}</Text>
          ))
        )}
      </Box>

      <Box flexDirection="column" marginTop={1} flexGrow={1}>
        <Text color="yellow">Live logs</Text>
        {logLines.length === 0 ? (
          <Text color="gray">{emptyMessage}</Text>
        ) : (
          logLines.map((line) => (
            <Text key={line.id}>{formatLogLine(line)}</Text>
          ))
        )}
      </Box>
      <Text color="gray">Type /help for commands</Text>
      <Text color="gray">Esc back, Ctrl+C exit</Text>
    </Box>
  );
}
