import fs from 'fs';

const Ledger = require('../../../src/ledger');

import { resolveClusterDbPath } from './cluster-logs';

export const MAX_TIMELINE_EVENTS = 40;
export const TIMELINE_POLL_INTERVAL_MS = 750;

export const WORKFLOW_TRIGGERS = Object.freeze([
  'ISSUE_OPENED',
  'PLAN_READY',
  'IMPLEMENTATION_READY',
  'VALIDATION_RESULT',
  'CONDUCTOR_ESCALATE',
]);

type ClusterTimelineState = 'idle' | 'waiting' | 'ready' | 'error';

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

function isWorkflowTopic(topic: string): boolean {
  return WORKFLOW_TRIGGERS.includes(topic);
}

function normalizeApproved(value: unknown): boolean | null {
  if (value === true || value === 'true') {
    return true;
  }
  if (value === false || value === 'false') {
    return false;
  }
  return null;
}

function labelForMessage(message: any, approved: boolean | null): string {
  switch (message.topic) {
    case 'ISSUE_OPENED':
      return 'Issue opened';
    case 'PLAN_READY':
      return 'Plan ready';
    case 'IMPLEMENTATION_READY':
      return 'Implementation ready';
    case 'VALIDATION_RESULT':
      if (approved === true) {
        return 'Validation approved';
      }
      if (approved === false) {
        return 'Validation rejected';
      }
      return 'Validation result';
    case 'CONDUCTOR_ESCALATE':
      return 'Conductor escalated';
    default:
      return message.topic || 'Workflow event';
  }
}

export function normalizeTimelineMessage(message: any): TimelineEvent | null {
  if (!message || typeof message !== 'object') {
    return null;
  }

  const topic = typeof message.topic === 'string' ? message.topic : '';
  if (!topic || !isWorkflowTopic(topic)) {
    return null;
  }

  const data = message.content?.data || {};
  const approved = normalizeApproved(data.approved);
  const timestamp = typeof message.timestamp === 'number' ? message.timestamp : Date.now();
  const id =
    typeof message.id === 'string'
      ? message.id
      : `${timestamp}-${Math.random().toString(16).slice(2)}`;

  return {
    id,
    timestamp,
    topic,
    label: labelForMessage(message, approved),
    approved,
    sender: typeof message.sender === 'string' ? message.sender : null,
  };
}

export function createClusterTimelineStream({
  clusterId,
  onEvents,
  onStatus,
  pollIntervalMs = TIMELINE_POLL_INTERVAL_MS,
  maxInitialEvents = MAX_TIMELINE_EVENTS,
}: ClusterTimelineStreamOptions) {
  let intervalId: NodeJS.Timeout | null = null;
  let ledger: any | null = null;
  let lastTimestamp = 0;
  let initialized = false;
  let closed = false;
  let lastStatus: ClusterTimelineStatus | null = null;

  const emitStatus = (status: ClusterTimelineStatus) => {
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
      resetLedger(err, 'Failed to open timeline database');
      return false;
    }
  };

  const queryWorkflowMessages = (since?: number): any[] => {
    if (!ledger || !clusterId) {
      return [];
    }
    const messages: any[] = [];
    const hasSince = typeof since === 'number' && Number.isFinite(since);
    for (const topic of WORKFLOW_TRIGGERS) {
      const criteria: any = {
        cluster_id: clusterId,
        topic,
        order: 'asc',
      };
      if (hasSince && since! > 0) {
        criteria.since = since;
      }
      try {
        const rows = ledger.query(criteria);
        if (rows.length) {
          messages.push(...rows);
        }
      } catch (err) {
        resetLedger(err, 'Failed to read timeline entries');
        return [];
      }
    }
    messages.sort((a, b) => (a.timestamp || 0) - (b.timestamp || 0));
    return messages;
  };

  const loadInitial = () => {
    if (!ledger || initialized || !clusterId) {
      return;
    }

    const messages = queryWorkflowMessages();
    if (messages.length) {
      const events = messages
        .map((message) => normalizeTimelineMessage(message))
        .filter(Boolean) as TimelineEvent[];
      if (events.length) {
        const trimmed =
          events.length > maxInitialEvents
            ? events.slice(events.length - maxInitialEvents)
            : events;
        onEvents(trimmed);
      }

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

    const since = lastTimestamp > 0 ? lastTimestamp + 1 : 1;
    const messages = queryWorkflowMessages(since);
    if (!messages.length) {
      return;
    }

    const events = messages
      .map((message) => normalizeTimelineMessage(message))
      .filter(Boolean) as TimelineEvent[];

    if (events.length) {
      onEvents(events);
    }

    const last = messages[messages.length - 1];
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
