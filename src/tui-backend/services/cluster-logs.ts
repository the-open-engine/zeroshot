import fs from 'fs';
import os from 'os';
import path from 'path';

const Ledger = require('../../../src/ledger');

export const MAX_LOG_LINES = 400;
export const LOG_POLL_INTERVAL_MS = 250;

type ClusterLogState = 'idle' | 'waiting' | 'ready' | 'error';

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

export function resolveClusterDbPath(clusterId: string): string {
  const envHome =
    (typeof process.env.HOME === 'string' && process.env.HOME.trim()) ||
    (typeof process.env.USERPROFILE === 'string' && process.env.USERPROFILE.trim()) ||
    (typeof process.env.HOMEDRIVE === 'string' &&
      typeof process.env.HOMEPATH === 'string' &&
      `${process.env.HOMEDRIVE}${process.env.HOMEPATH}`.trim()) ||
    '';
  const homeDir = envHome || os.homedir();
  const storageDir = path.join(homeDir, '.zeroshot');
  const clustersFile = path.join(storageDir, 'clusters.json');

  try {
    if (fs.existsSync(clustersFile)) {
      const raw = fs.readFileSync(clustersFile, 'utf8');
      try {
        const data = JSON.parse(raw);
        const entry = data && typeof data === 'object' ? data[clusterId] : null;
        const dbPath = entry?.config?.dbPath;

        if (typeof dbPath === 'string' && dbPath.trim()) {
          return dbPath;
        }
      } catch {
        // clusters.json can be mid-write; fall back to default path
      }
    }
  } catch {
    // Ignore fs errors; fall back to default path
  }

  return path.join(storageDir, `${clusterId}.db`);
}

export function normalizeAgentOutput(message: any): ClusterLogLine | null {
  if (!message || typeof message !== 'object') {
    return null;
  }

  const content = message.content || {};
  const data = content.data || {};
  const contentText = typeof content.text === 'string' ? content.text : '';
  const dataLine = typeof data.line === 'string' ? data.line : '';
  const text = contentText.trim() ? contentText : dataLine;

  if (!text || !text.trim()) {
    return null;
  }

  const timestamp = typeof message.timestamp === 'number' ? message.timestamp : Date.now();
  const sender = typeof message.sender === 'string' ? message.sender : null;
  const agent = typeof data.agent === 'string' ? data.agent : sender;
  const role = typeof data.role === 'string' ? data.role : null;
  const id =
    typeof message.id === 'string'
      ? message.id
      : `${timestamp}-${Math.random().toString(16).slice(2)}`;

  return {
    id,
    timestamp,
    text,
    agent,
    role,
    sender,
  };
}

export function createClusterLogStream({
  clusterId,
  agentId,
  onLines,
  onStatus,
  pollIntervalMs = LOG_POLL_INTERVAL_MS,
  maxInitialLines = MAX_LOG_LINES,
}: ClusterLogStreamOptions) {
  let intervalId: NodeJS.Timeout | null = null;
  let ledger: any | null = null;
  let lastTimestamp = 0;
  let initialized = false;
  let closed = false;
  let lastStatus: ClusterLogStatus | null = null;
  const normalizedAgentId = typeof agentId === 'string' && agentId.trim() ? agentId.trim() : null;

  const filterLines = (lines: ClusterLogLine[]) => {
    if (!normalizedAgentId) {
      return lines;
    }
    return lines.filter(
      (line) => line.agent === normalizedAgentId || line.sender === normalizedAgentId
    );
  };

  const emitStatus = (status: ClusterLogStatus) => {
    if (!onStatus) return;
    if (lastStatus && lastStatus.state === status.state && lastStatus.message === status.message) {
      return;
    }
    lastStatus = status;
    onStatus(status);
  };

  const emitError = (err: unknown, context: string) => {
    const message =
      err instanceof Error ? err.message : typeof err === 'string' ? err : 'Unknown error';
    emitStatus({
      state: 'error',
      message: context ? `${context}: ${message}` : message,
    });
  };

  const resetLedger = (err: unknown, context: string) => {
    emitError(err, context);
    if (ledger) {
      try {
        ledger.close();
      } catch {
        // ignore close errors
      }
      ledger = null;
    }
  };

  const ensureLedger = () => {
    if (!clusterId) {
      emitStatus({ state: 'idle' });
      return false;
    }

    if (ledger) {
      return true;
    }

    const dbPath = resolveClusterDbPath(clusterId);
    if (!fs.existsSync(dbPath)) {
      emitStatus({ state: 'waiting' });
      return false;
    }

    try {
      ledger = new Ledger(dbPath);
      if (!initialized) {
        lastTimestamp = 0;
      }
      emitStatus({ state: 'ready' });
      return true;
    } catch (err) {
      resetLedger(err, 'Failed to open log database');
      return false;
    }
  };

  const loadInitial = () => {
    if (!ledger || initialized || !clusterId) {
      return;
    }

    let rows: any[] = [];
    try {
      rows = ledger.query({
        cluster_id: clusterId,
        topic: 'AGENT_OUTPUT',
        order: 'desc',
        limit: maxInitialLines,
      });
    } catch (err) {
      resetLedger(err, 'Failed to read initial logs');
      return;
    }

    const messages = rows.slice().reverse();
    const lines = filterLines(
      messages
        .map((message: any) => normalizeAgentOutput(message))
        .filter(Boolean) as ClusterLogLine[]
    );

    if (lines.length > 0) {
      onLines(lines);
    }

    if (messages.length > 0) {
      const last = messages[messages.length - 1];
      if (last && typeof last.timestamp === 'number') {
        lastTimestamp = Math.max(lastTimestamp, last.timestamp);
      }
    }

    initialized = true;
  };

  const poll = () => {
    if (closed) {
      return;
    }

    if (!clusterId) {
      emitStatus({ state: 'idle' });
      return;
    }

    if (!ensureLedger()) {
      return;
    }

    loadInitial();

    let rows: any[] = [];
    try {
      rows = ledger.query({
        cluster_id: clusterId,
        topic: 'AGENT_OUTPUT',
        since: lastTimestamp + 1,
        order: 'asc',
      });
    } catch (err) {
      resetLedger(err, 'Failed to read logs');
      return;
    }

    if (!rows.length) {
      return;
    }

    const lines = filterLines(
      rows.map((message: any) => normalizeAgentOutput(message)).filter(Boolean) as ClusterLogLine[]
    );

    if (lines.length > 0) {
      onLines(lines);
    }

    const last = rows[rows.length - 1];
    if (last && typeof last.timestamp === 'number') {
      lastTimestamp = Math.max(lastTimestamp, last.timestamp);
    }
  };

  const start = () => {
    if (intervalId) {
      return;
    }
    poll();
    intervalId = setInterval(poll, pollIntervalMs);
  };

  const stop = () => {
    if (!intervalId) {
      return;
    }
    clearInterval(intervalId);
    intervalId = null;
  };

  const close = () => {
    closed = true;
    stop();
    if (ledger) {
      ledger.close();
      ledger = null;
    }
  };

  return { start, stop, close };
}
