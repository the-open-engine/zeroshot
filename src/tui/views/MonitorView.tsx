import React, { useEffect, useMemo, useState } from "react";
import { Box, Text, useInput } from "ink";
import {
  ClusterSummary,
  ClusterMetrics,
  listClusterMetrics,
  listClusters,
} from "../services/cluster-registry";

type MonitorViewProps = {
  provider: string | null;
  onOpenCluster?: (clusterId: string) => void;
  isCommandInputEmpty?: boolean;
};

const REFRESH_INTERVAL_MS = 3000;
const METRICS_REFRESH_INTERVAL_MS = 2000;

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

function formatCpu(cpuPercent: number | null | undefined): string {
  if (!Number.isFinite(cpuPercent)) {
    return "-";
  }
  return `${Number(cpuPercent).toFixed(1)}%`;
}

function formatMemory(memoryMB: number | null | undefined): string {
  if (!Number.isFinite(memoryMB)) {
    return "-";
  }
  return `${Math.round(Number(memoryMB))}MB`;
}

export default function MonitorView({
  provider,
  onOpenCluster,
  isCommandInputEmpty = false,
}: MonitorViewProps) {
  const [clusters, setClusters] = useState<ClusterSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [metricsById, setMetricsById] = useState<
    Record<string, ClusterMetrics>
  >({});
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;

    async function refresh() {
      setIsLoading(true);
      try {
        const result = await listClusters();
        if (cancelled) {
          return;
        }
        setClusters(result);
        setSelectedId((current) => {
          if (result.length === 0) {
            return null;
          }
          if (current && result.some((cluster) => cluster.id === current)) {
            return current;
          }
          return result[0].id;
        });
        setError(null);
      } catch (err) {
        if (cancelled) {
          return;
        }
        const message = err instanceof Error ? err.message : "Failed to load clusters.";
        setError(message);
      } finally {
        if (!cancelled) {
          setIsLoading(false);
        }
      }
    }

    void refresh();
    const interval = setInterval(refresh, REFRESH_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let inFlight = false;

    async function refreshMetrics() {
      if (inFlight) {
        return;
      }
      inFlight = true;
      try {
        const result = await listClusterMetrics();
        if (!cancelled) {
          setMetricsById(result);
        }
      } catch {
        if (!cancelled) {
          setMetricsById({});
        }
      } finally {
        inFlight = false;
      }
    }

    void refreshMetrics();
    const interval = setInterval(refreshMetrics, METRICS_REFRESH_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, []);

  const rows = useMemo(() => {
    const now = Date.now();
    return clusters.map((cluster) => ({
      ...cluster,
      age: formatAge(cluster.createdAt, now),
      cwdDisplay: cluster.cwd ?? "-",
      cpuDisplay: formatCpu(metricsById[cluster.id]?.cpuPercent),
      memoryDisplay: formatMemory(metricsById[cluster.id]?.memoryMB),
    }));
  }, [clusters, metricsById]);

  const columnWidths = useMemo(() => {
    const idWidth = clamp(
      rows.reduce((max, row) => Math.max(max, row.id.length), 2),
      8,
      24
    );
    const statusWidth = clamp(
      rows.reduce((max, row) => Math.max(max, row.state.length), 6),
      6,
      14
    );
    const ageWidth = clamp(
      rows.reduce((max, row) => Math.max(max, row.age.length), 3),
      3,
      6
    );
    const cpuWidth = clamp(
      rows.reduce((max, row) => Math.max(max, row.cpuDisplay.length), 4),
      4,
      8
    );
    const memWidth = clamp(
      rows.reduce((max, row) => Math.max(max, row.memoryDisplay.length), 3),
      3,
      10
    );
    return { idWidth, statusWidth, ageWidth, cpuWidth, memWidth };
  }, [rows]);

  useInput((_input, key) => {
    if (clusters.length === 0) {
      return;
    }
    const currentIndex = Math.max(
      0,
      clusters.findIndex((cluster) => cluster.id === selectedId)
    );

    if (key.upArrow) {
      const nextIndex = Math.max(0, currentIndex - 1);
      setSelectedId(clusters[nextIndex].id);
      return;
    }
    if (key.downArrow) {
      const nextIndex = Math.min(clusters.length - 1, currentIndex + 1);
      setSelectedId(clusters[nextIndex].id);
      return;
    }
    if (key.return && isCommandInputEmpty) {
      const cluster = clusters[currentIndex];
      if (cluster && onOpenCluster) {
        onOpenCluster(cluster.id);
      }
    }
  });

  return (
    <Box flexDirection="column">
      <Text color="cyan">Monitor</Text>
      <Text color="gray">Provider: {provider || "auto"}</Text>
      {error ? (
        <Text color="red">Error: {error}</Text>
      ) : null}
      {isLoading && clusters.length === 0 ? (
        <Text color="gray">Loading clusters...</Text>
      ) : null}
      {!isLoading && clusters.length === 0 && !error ? (
        <Text color="gray">No clusters found.</Text>
      ) : null}
      {clusters.length > 0 ? (
        <Box flexDirection="column" marginTop={1}>
          <Text color="gray">
            {padRight("ID", columnWidths.idWidth)}{" "}
            {padRight("Status", columnWidths.statusWidth)}{" "}
            {padRight("Age", columnWidths.ageWidth)}{" "}
            {padRight("CPU%", columnWidths.cpuWidth)}{" "}
            {padRight("Mem", columnWidths.memWidth)} CWD
          </Text>
          {rows.map((row) => {
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
