import React, { useEffect, useMemo, useRef, useState } from "react";
import { Box, Text, useInput } from "ink";
import { listClusters } from "../services/cluster-registry";
import {
  fetchMonitorMetrics,
  MonitorClusterRow,
} from "../services/monitor-metrics";

type MonitorViewProps = {
  provider: string | null;
  onOpenCluster?: (clusterId: string) => void;
  isCommandInputEmpty?: boolean;
};

const REFRESH_INTERVAL_MS = 2000;

function formatAge(createdAt: number, now: number): string {
  const diffSeconds = Math.max(0, Math.floor((now - createdAt) / 1000));
  if (diffSeconds < 60) {
    return `${diffSeconds}s`;
  }
  const minutes = Math.floor(diffSeconds / 60);
  if (minutes < 60) {
    return `${minutes}m`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  return `${days}d`;
}

function padRight(text: string, width: number): string {
  if (text.length >= width) {
    return text.slice(0, width);
  }
  return text.padEnd(width, " ");
}

function truncate(text: string, width: number): string {
  if (text.length <= width) {
    return text;
  }
  if (width <= 3) {
    return text.slice(0, width);
  }
  return `${text.slice(0, width - 3)}...`;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

function formatCpu(cpu: number | null): string {
  if (cpu === null) return "--";
  return `${cpu.toFixed(1)}%`;
}

function formatMemory(bytes: number | null): string {
  if (bytes === null) return "--";
  if (!Number.isFinite(bytes) || bytes <= 0) return "0B";
  const kb = 1024;
  const mb = kb * 1024;
  const gb = mb * 1024;
  if (bytes < kb) return `${Math.round(bytes)}B`;
  if (bytes < mb) return `${(bytes / kb).toFixed(1)}KB`;
  if (bytes < gb) return `${(bytes / mb).toFixed(1)}MB`;
  return `${(bytes / gb).toFixed(1)}GB`;
}

export default function MonitorView({
  provider,
  onOpenCluster,
  isCommandInputEmpty = false,
}: MonitorViewProps) {
  const [rows, setRows] = useState<MonitorClusterRow[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const inFlightRef = useRef(false);

  useEffect(() => {
    let cancelled = false;

    const refresh = async () => {
      if (inFlightRef.current) return;
      inFlightRef.current = true;
      setIsLoading(true);

      try {
        const clusters = await listClusters();
        let metricsResult = await fetchMonitorMetrics({ clusters });
        if (!metricsResult.rows.length) {
          metricsResult = {
            rows: clusters.map((cluster) => ({
              ...cluster,
              cpu: null,
              memory: null,
            })),
            error: metricsResult.error,
          };
        }

        if (!cancelled) {
          setRows(metricsResult.rows);
          setSelectedId((current) => {
            if (metricsResult.rows.length === 0) {
              return null;
            }
            if (current && metricsResult.rows.some((row) => row.id === current)) {
              return current;
            }
            return metricsResult.rows[0].id;
          });
          setError(metricsResult.error);
        }
      } catch (err) {
        if (!cancelled) {
          const message =
            err instanceof Error ? err.message : "Failed to load clusters.";
          setError(message);
        }
      } finally {
        if (!cancelled) {
          setIsLoading(false);
        }
        inFlightRef.current = false;
      }
    };

    void refresh();
    const interval = setInterval(refresh, REFRESH_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, []);

  const displayRows = useMemo(() => {
    const now = Date.now();
    return rows.map((row) => ({
      ...row,
      age: formatAge(row.createdAt, now),
      cwdDisplay: row.cwd ?? "-",
      cpuDisplay: formatCpu(row.cpu),
      memoryDisplay: formatMemory(row.memory),
    }));
  }, [rows]);

  const columnWidths = useMemo(() => {
    const idWidth = clamp(
      displayRows.reduce((max, row) => Math.max(max, row.id.length), 2),
      8,
      24
    );
    const statusWidth = clamp(
      displayRows.reduce((max, row) => Math.max(max, row.state.length), 6),
      6,
      14
    );
    const ageWidth = clamp(
      displayRows.reduce((max, row) => Math.max(max, row.age.length), 3),
      3,
      6
    );
    const cpuWidth = clamp(
      displayRows.reduce((max, row) => Math.max(max, row.cpuDisplay.length), 3),
      4,
      8
    );
    const memWidth = clamp(
      displayRows.reduce((max, row) => Math.max(max, row.memoryDisplay.length), 3),
      5,
      10
    );
    return { idWidth, statusWidth, ageWidth, cpuWidth, memWidth };
  }, [displayRows]);

  useInput((_input, key) => {
    if (rows.length === 0) {
      return;
    }
    const currentIndex = Math.max(
      0,
      rows.findIndex((row) => row.id === selectedId)
    );

    if (key.upArrow) {
      const nextIndex = Math.max(0, currentIndex - 1);
      setSelectedId(rows[nextIndex].id);
      return;
    }
    if (key.downArrow) {
      const nextIndex = Math.min(rows.length - 1, currentIndex + 1);
      setSelectedId(rows[nextIndex].id);
      return;
    }
    if (key.return && isCommandInputEmpty) {
      const row = rows[currentIndex];
      if (row && onOpenCluster) {
        onOpenCluster(row.id);
      }
    }
  });

  return (
    <Box flexDirection="column">
      <Text color="cyan">Monitor</Text>
      <Text color="gray">Provider: {provider || "auto"}</Text>
      <Text color="gray">Refresh: {REFRESH_INTERVAL_MS / 1000}s</Text>
      {error ? <Text color="yellow">{error}</Text> : null}
      {isLoading && rows.length === 0 ? (
        <Text color="gray">Loading clusters...</Text>
      ) : null}
      {!isLoading && rows.length === 0 && !error ? (
        <Text color="gray">No clusters found.</Text>
      ) : null}
      {rows.length > 0 ? (
        <Box flexDirection="column" marginTop={1}>
          <Text color="gray">
            {padRight("ID", columnWidths.idWidth)}{" "}
            {padRight("Status", columnWidths.statusWidth)}{" "}
            {padRight("Age", columnWidths.ageWidth)}{" "}
            {padRight("CPU", columnWidths.cpuWidth)}{" "}
            {padRight("Mem", columnWidths.memWidth)} CWD
          </Text>
          {displayRows.map((row) => {
            const isSelected = row.id === selectedId;
            const line = [
              padRight(row.id, columnWidths.idWidth),
              padRight(row.state, columnWidths.statusWidth),
              padRight(row.age, columnWidths.ageWidth),
              padRight(row.cpuDisplay, columnWidths.cpuWidth),
              padRight(row.memoryDisplay, columnWidths.memWidth),
              truncate(row.cwdDisplay, 80),
            ].join(" ");
            return (
              <Text key={row.id} inverse={isSelected}>
                {line}
              </Text>
            );
          })}
        </Box>
      ) : null}
      <Text color="gray">
        Keys: Up/Down select, Enter open (empty input), Esc back
      </Text>
    </Box>
  );
}
