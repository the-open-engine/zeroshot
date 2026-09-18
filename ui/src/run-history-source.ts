import { ApiError, type ApiClient } from './api';
import { watchHistoryEvents, type HistoryObserver } from './history-stream';
import {
  historyCursorSequence as cursorSequence,
  invalidRuntimeFailure,
  readPageRuntimeFailure,
  readRuntimeFailure,
} from './history-contract';
import type { HistoryEvent, HistoryPage, RunDetail, RunSummary } from './run-history';

/** A viewer needs a selected run, not knowledge of its host's run menu. */
export interface RunHistoryReader {
  detail(id: string, signal?: AbortSignal): Promise<RunDetail>;
  page(id: string, after: string, signal?: AbortSignal): Promise<HistoryPage>;
  watch?(id: string, after: string, observer: HistoryObserver, signal?: AbortSignal): () => void;
}
export interface RunHistorySource extends RunHistoryReader {
  list(
    after?: string,
    signal?: AbortSignal
  ): Promise<{ runs: RunSummary[]; nextCursor: string | null }>;
}

/** A definition and its projection must agree before any history is replayed. */
export function readRunDetail(value: unknown, requestedRunId: string): RunDetail {
  const detail = value as Partial<RunDetail> | null;
  if (detail?.version !== 1 || detail?.projectionVersion !== 1)
    throw new ApiError(
      200,
      'incompatible_history',
      'This run uses incompatible definition or history versions. Reload after updating Zeroshot.'
    );
  if (detail.runId !== requestedRunId)
    throw new ApiError(
      200,
      'run_identity_mismatch',
      'The server returned a different run. Reload to try again.'
    );
  const failure = readRuntimeFailure(detail.runtimeFailure, detail.history?.cursor);
  if (
    failure &&
    (detail.phase !== 'finished' ||
      detail.terminal?.status !== 'failed' ||
      detail.terminal.reason !== failure.reason ||
      detail.history?.complete !== false ||
      detail.cursor !== detail.history.cursor)
  )
    throw invalidRuntimeFailure();
  return detail as RunDetail;
}

export function createRunHistorySource(api: ApiClient, base: URL): RunHistorySource {
  return {
    list: (after, signal) =>
      api(`runs${after ? `?after=${encodeURIComponent(after)}` : ''}`, undefined, signal),
    detail: async (id, signal) =>
      readRunDetail(await api(`runs/${encodeURIComponent(id)}`, undefined, signal), id),
    page: async (id, after, signal) => {
      const page = await api<HistoryPage>(
        `runs/${encodeURIComponent(id)}/history?after=${encodeURIComponent(after)}`,
        undefined,
        signal
      );
      readPageRuntimeFailure(page);
      return page;
    },
    watch: (id, after, observer, signal) =>
      watchHistoryEvents(
        new URL(`runs/${encodeURIComponent(id)}/events?after=${encodeURIComponent(after)}`, base),
        observer,
        signal
      ),
  };
}
type Example = { detail: RunDetail; events: HistoryEvent[] };
export function createExampleRunHistory(
  mount: URL,
  fetcher: typeof fetch = fetch
): RunHistorySource {
  let examplesPromise: Promise<Example[]> | undefined;
  async function examples(): Promise<Example[]> {
    if (!examplesPromise)
      examplesPromise = fetcher(new URL('./history-examples.json', mount), {
        cache: 'no-store',
      })
        .then(async (response) => {
          if (!response.ok) throw new Error('Examples could not be loaded.');
          return (await response.json()) as Example[];
        })
        .catch((error) => {
          examplesPromise = undefined;
          throw error;
        });
    return examplesPromise;
  }
  return {
    async list() {
      return {
        runs: (await examples()).map(({ detail }) => ({ ...detail, example: true })),
        nextCursor: null,
      };
    },
    async detail(id) {
      const example = (await examples()).find(({ detail }) => detail.runId === id);
      if (!example) throw new Error('Example not found.');
      return { ...readRunDetail(example.detail, id), example: true };
    },
    async page(id, after) {
      const example = (await examples()).find(({ detail }) => detail.runId === id);
      if (!example) throw new Error('Example not found.');
      const start =
        after === 'v2:0' ? 0 : example.events.findIndex((event) => event.cursor === after) + 1;
      if (after !== 'v2:0' && start === 0) throw new Error('Unknown example cursor.');
      const events = example.events.slice(start, start + 256);
      return {
        events,
        nextCursor: events.at(-1)?.cursor ?? after,
        headCursor: example.detail.history.cursor,
        complete: start + events.length >= example.events.length,
      };
    },
  };
}

/** Merge a contiguous replay prefix, accepting identical retransmitted events only. */
export function appendHistory(
  previous: readonly HistoryEvent[],
  page: HistoryPage
): HistoryEvent[] {
  readPageRuntimeFailure(page);
  const seen = new Map<string, HistoryEvent>();
  let sequence = 0n;
  for (const event of previous) {
    if (cursorSequence(event.cursor) !== sequence + 1n) throw historyPageError();
    seen.set(event.cursor, event);
    sequence++;
  }
  const next = [...previous];
  let pageSequence = -1n;
  for (const event of page.events) {
    const current = cursorSequence(event.cursor);
    if (current < pageSequence) throw historyPageError();
    pageSequence = current;
    const existing = seen.get(event.cursor);
    if (existing) {
      if (JSON.stringify(existing) !== JSON.stringify(event)) throw historyPageError();
      continue;
    }
    if (current !== sequence + 1n) throw historyPageError();
    seen.set(event.cursor, event);
    next.push(event);
    sequence = current;
  }
  const tail = cursorSequence(page.nextCursor);
  const head = cursorSequence(page.headCursor);
  if (
    tail !== sequence ||
    (pageSequence >= 0n && pageSequence !== tail) ||
    head < tail ||
    page.complete !== (head === tail)
  ) {
    throw historyPageError();
  }
  return next;
}

function historyPageError(): Error {
  return new Error('History contains a gap or inconsistent cursor. Reload to try again.');
}
