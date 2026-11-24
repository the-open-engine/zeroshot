import { historyCursorSequence as sequence, validateFailureContext } from './history-contract';
import type { ControlRecord } from './control-history';
import type { HistoryEvent, HistoryPage } from './run-history';

export const runIdPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
export function runPosition(value: string): { part: string; cursor: string } | undefined {
  const split = value.indexOf('@');
  if (split < 0) return;
  const part = value.slice(0, split),
    cursor = value.slice(split + 1);
  if (!runIdPattern.test(part)) throw invalid();
  sequence(cursor);
  return { part, cursor };
}

function invalid(): Error {
  return new Error('History contains a gap or inconsistent cursor. Reload to try again.');
}

function native(cursor: string | undefined): bigint {
  if (!cursor?.startsWith('v2:')) throw invalid();
  return sequence(cursor);
}

function validWindow(page: HistoryPage, part: string) {
  const lower = native(page.partAfterCursor),
    upper = native(page.partNextCursor);
  const physicalHead = native(page.partHeadCursor);
  if (upper < lower || upper > physicalHead || upper - lower > 256n) throw invalid();
  const head = runPosition(page.headCursor);
  if (head?.part !== page.currentPartId) throw invalid();
  const current = part === head!.part;
  if (current && physicalHead !== native(head!.cursor)) throw invalid();
  if (page.complete !== (current && upper === physicalHead)) throw invalid();
  const next = runPosition(page.nextCursor);
  if (!next) throw invalid();
  if (
    next.part === part
      ? next.cursor !== page.partNextCursor
      : next.cursor !== 'v2:0' || upper !== physicalHead
  )
    throw invalid();
  return { lower, upper };
}

function normalizeRecord(record: HistoryEvent, part: string, covered: Set<bigint>): HistoryEvent {
  const at = native(record.cursor);
  if (covered.has(at) || record.partId !== part || record.runCursor !== `${part}@${record.cursor}`)
    throw invalid();
  covered.add(at);
  const time = record.recordedAt;
  if (time !== undefined && (!Number.isSafeInteger(time) || time <= 0)) throw invalid();
  if (record.event.kind === 'terminal' && record.event.result?.status === 'failed')
    validateFailureContext(record.event.result.context, record.cursor);
  return { ...record, cursor: record.runCursor, partCursor: record.cursor };
}

function controlPositions(page: HistoryPage, part: string): HistoryEvent[] {
  // The reducer can anchor a transition to a copied vector. Retain the position while
  // showing the operator's original request or answer just once.
  const omitted = new Set(page.omittedCursors);
  return Array.from(new Set(page.importedControl?.map((record) => record.cursor)))
    .filter((cursor) => omitted.has(cursor))
    .map((cursor) => ({
      cursor: `${part}@${cursor}`,
      partId: part,
      partCursor: cursor,
      event: { kind: 'control_position' },
    }));
}

/** Normalize directly addressable records from a grouped page to a composite local key.
 * Exact part archives and example data keep their native cursors. */
export function groupedRecords(page: HistoryPage): HistoryEvent[] {
  const part = page.partId;
  if (!part || ![part, page.runId, page.currentPartId].every((id) => id && runIdPattern.test(id)))
    throw invalid();
  const { lower, upper } = validWindow(page, part);
  const covered = new Set(page.omittedCursors?.map(native));
  const events = [...page.events, ...(page.imports ?? [])].map((record) =>
    normalizeRecord(record, part, covered)
  );
  if (
    BigInt(covered.size) !== upper - lower ||
    [...covered].some((index) => index <= lower || index > upper)
  )
    throw invalid();
  return events
    .concat(controlPositions(page, part))
    .sort((a, b) => Number(native(a.partCursor) - native(b.partCursor)));
}

function requireSameRecord(old: HistoryEvent, incoming: HistoryEvent) {
  if (JSON.stringify(old) !== JSON.stringify(incoming)) throw invalid();
}

/** Each page consumes an exact native interval, including imported or omitted records. */
export function appendGroupedHistory(
  previous: readonly HistoryEvent[],
  page: HistoryPage,
  expected?: string
): HistoryEvent[] {
  const records = groupedRecords(page);
  if (
    expected &&
    expected !==
      (expected.includes('@') ? `${page.partId}@${page.partAfterCursor}` : page.partAfterCursor)
  )
    throw invalid();
  const seen = new Map(previous.map((event) => [event.cursor, event]));
  let recordedAt =
    previous.at(-1)?.partId === page.partId ? previous.at(-1)?.recordedAt : undefined;
  const added: HistoryEvent[] = [];
  for (const event of records) {
    const old = seen.get(event.cursor);
    if (old) {
      requireSameRecord(old, event);
      continue;
    }
    if (recordedAt !== undefined && event.recordedAt !== undefined && event.recordedAt < recordedAt)
      throw invalid();
    recordedAt = event.recordedAt ?? recordedAt;
    added.push(event);
  }
  return [...previous, ...added];
}

export function groupedControlPage(page: HistoryPage): HistoryPage {
  const events = groupedRecords(page);
  const position = new Map(events.map((record, index) => [record.cursor, index]));
  const control = [...(page.control ?? []), ...(page.importedControl ?? [])].map(
    (record): ControlRecord => {
      if (record.partId !== page.partId || record.runCursor !== `${page.partId}@${record.cursor}`)
        throw invalid();
      return { ...record, cursor: record.runCursor };
    }
  );
  control.sort(
    (left, right) => (position.get(left.cursor) ?? -1) - (position.get(right.cursor) ?? -1)
  );
  return { ...page, events, control };
}
