import React, { useEffect, useMemo, useRef, useState } from "react";
import { Box, Text } from "ink";
import {
  ClusterLogLine,
  ClusterLogStatus,
  createClusterLogStream,
  MAX_LOG_LINES,
} from "../services/cluster-logs";

type ClusterViewProps = {
  provider: string | null;
  clusterId?: string | null;
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

export default function ClusterView({ provider, clusterId }: ClusterViewProps) {
  const [logLines, setLogLines] = useState<ClusterLogLine[]>([]);
  const [logStatus, setLogStatus] = useState<ClusterLogStatus>(EMPTY_STATUS);
  const streamRef = useRef<ClusterLogStreamHandle | null>(null);

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

  return (
    <Box flexDirection="column">
      <Box flexDirection="column" marginBottom={1}>
        <Text color="cyan">Cluster</Text>
        <Text color="gray">Id: {clusterId || "pending"}</Text>
        <Text color="gray">Provider: {provider || "auto"}</Text>
      </Box>

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
        <Text color="gray">Esc back, Ctrl+C exit</Text>
      </Box>
    </Box>
  );
}
