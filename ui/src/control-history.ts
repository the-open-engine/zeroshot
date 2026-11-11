import { pathTo, type GraphNode } from './domain';
import type { HistoryEvent, HistoryPage } from './run-history';
import type { WorkflowNodeObservation } from './workflow-observation';

/** Derived by the canonical reducer at a durable cursor; these are not worker executions. */
export type ControlRecord = {
  cursor: string;
  node: string;
  mapIndices: number[];
  visitId: string;
  state: 'queued' | 'entered' | 'completed' | 'succeeded' | 'failed' | 'stopped';
  branch?: string;
  detail?: string;
  output?: unknown;
};
export type ControlVisit = ControlRecord & {
  start: number;
  updated: number;
  queuedAt?: number;
  visit: number;
};

/** A reducer `completed` visit is neutral: it also marks groups that a fail node ended. */
export function controlState(state: ControlRecord['state']): WorkflowNodeObservation['state'] {
  if (state === 'queued') return 'queued';
  return state === 'entered'
    ? 'running'
    : state === 'succeeded' || state === 'failed'
      ? state
      : state === 'stopped'
        ? 'skipped'
        : 'recorded';
}

/**
 * A seq, choice, loop or map ends in the same reduction as a succeed or fail node it contains; a
 * par never passes a branch terminal up. Give those enclosing visits the terminal's outcome; every
 * other completed visit stays neutral.
 */
export function settleControlVisits(
  root: GraphNode,
  visits: readonly ControlVisit[]
): ControlVisit[] {
  const outcomes = new Map<ControlVisit, 'succeeded' | 'failed'>();
  for (const [position, terminal] of visits.entries()) {
    if (terminal.state !== 'succeeded' && terminal.state !== 'failed') continue;
    for (const group of pathTo(root, terminal.node).slice(0, -1).reverse()) {
      if (group.kind === 'par') break;
      // Visits are in entry order, so the group's nearest earlier visit in the terminal's map scope
      // holds it, even when later loop rounds below a par end in the same reduction.
      let enclosing: ControlVisit | undefined;
      for (let index = position - 1; index >= 0 && !enclosing; index--) {
        const visit = visits[index];
        if (
          visit.node === group.name &&
          visit.mapIndices.every((item, level) => item === terminal.mapIndices[level])
        )
          enclosing = visit;
      }
      if (enclosing?.state === 'completed' && outcomes.get(enclosing) !== 'failed')
        outcomes.set(enclosing, terminal.state);
    }
  }
  return visits.map((visit) => {
    const state = outcomes.get(visit);
    return state ? { ...visit, state } : visit;
  });
}

export function controlVisitLabel(visit: ControlVisit): string {
  return [
    ...visit.mapIndices.map(
      (item, level) =>
        `${visit.mapIndices.length > 1 ? `Map ${level + 1}, item` : 'Item'} ${item + 1}`
    ),
    `Visit ${visit.visit}`,
  ].join(' · ');
}

function validControlRecord(record: ControlRecord): boolean {
  return (
    !!record &&
    typeof record.node === 'string' &&
    typeof record.visitId === 'string' &&
    Array.isArray(record.mapIndices) &&
    record.mapIndices.every((index) => Number.isSafeInteger(index) && index >= 0) &&
    ['queued', 'entered', 'completed', 'succeeded', 'failed', 'stopped'].includes(record.state) &&
    (record.branch === undefined || typeof record.branch === 'string') &&
    (record.detail === undefined || typeof record.detail === 'string')
  );
}

/** Merge only reducer records anchored in this page. Retransmission must be identical. */
export function appendControlHistory(
  previous: readonly ControlRecord[],
  page: HistoryPage,
  acceptedEvents: readonly HistoryEvent[] = []
): ControlRecord[] {
  if (page.control === undefined) return [...previous];
  const fail = () => {
    throw new Error('Control history is inconsistent. Worker history remains available.');
  };
  if (!Array.isArray(page.control)) return fail();
  const positions = new Map(page.events.map((event, index) => [event.cursor, index]));
  const accepted = new Set(acceptedEvents.map((event) => event.cursor));
  const identities = new Map(
    previous.map((record) => [record.visitId, JSON.stringify([record.node, record.mapIndices])])
  );
  let position = -1;
  for (const record of page.control) {
    if (!validControlRecord(record)) return fail();
    const next = positions.get(record.cursor);
    if (next === undefined || next < position) return fail();
    const identity = JSON.stringify([record.node, record.mapIndices]);
    if (identities.has(record.visitId) && identities.get(record.visitId) !== identity)
      return fail();
    identities.set(record.visitId, identity);
    position = next;
  }
  const oldByCursor = new Map<string, ControlRecord[]>();
  for (const record of previous) {
    accepted.add(record.cursor);
    const records = oldByCursor.get(record.cursor) ?? [];
    records.push(record);
    oldByCursor.set(record.cursor, records);
  }
  const incoming = new Map<string, ControlRecord[]>();
  for (const record of page.control) {
    const records = incoming.get(record.cursor) ?? [];
    records.push(record);
    incoming.set(record.cursor, records);
  }
  for (const cursor of positions.keys()) {
    const old = oldByCursor.get(cursor);
    if (
      accepted.has(cursor) &&
      JSON.stringify(old ?? []) !== JSON.stringify(incoming.get(cursor) ?? [])
    )
      return fail();
  }
  return [...previous, ...page.control.filter((record) => !accepted.has(record.cursor))];
}

/** Project only the selected ledger prefix. Never expose a later route or terminal result. */
export function projectControlHistory(
  records: readonly ControlRecord[],
  events: readonly HistoryEvent[],
  through: number
): ControlVisit[] {
  const positions = new Map(
    events.slice(0, through + 1).map((event, index) => [event.cursor, index])
  );
  const visits = new Map<string, ControlVisit>();
  const counts = new Map<string, number>();
  for (const record of records) {
    const index = positions.get(record.cursor);
    if (index === undefined) continue;
    const previous = visits.get(record.visitId);
    const scope = JSON.stringify([record.node, record.mapIndices]);
    const visit = previous?.visit ?? (counts.get(scope) ?? 0) + 1;
    counts.set(scope, Math.max(counts.get(scope) ?? 0, visit));
    visits.set(record.visitId, {
      ...previous,
      ...record,
      visit,
      start: previous?.start ?? index,
      updated: index,
      queuedAt: record.state === 'queued' ? index : previous?.queuedAt,
    });
  }
  return [...visits.values()];
}
