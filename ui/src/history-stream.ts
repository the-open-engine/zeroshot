import type { HistoryPage } from './run-history';
import { readPageRuntimeFailure } from './history-contract';

export type HistoryConnection = 'connecting' | 'connected' | 'reconnecting';
export type HistoryObserver = {
  page(page: HistoryPage): void;
  status(state: HistoryConnection): void;
  error(error: Error): void;
};

type HistoryEventStream = {
  readonly readyState: number;
  addEventListener(type: string, listener: EventListener): void;
  removeEventListener(type: string, listener: EventListener): void;
  close(): void;
};
type EventStreamFactory = (url: URL) => HistoryEventStream;

/** The browser retains Last-Event-ID across reconnects; invalid history never reconnects. */
export function watchHistoryEvents(
  url: URL,
  observer: HistoryObserver,
  signal?: AbortSignal,
  create: EventStreamFactory = (url) => new EventSource(url)
): () => void {
  let source: HistoryEventStream | undefined;
  let closed = false;
  let status: HistoryConnection | undefined;
  const dispose = () => {
    if (closed) return;
    closed = true;
    signal?.removeEventListener('abort', dispose);
    if (source) {
      source.removeEventListener('open', onOpen);
      source.removeEventListener('error', onDisconnect);
      source.removeEventListener('history', onHistory);
      source.removeEventListener('history_error', onHistoryError);
      source.close();
    }
  };
  const fail = (cause: unknown) => {
    if (closed) return;
    dispose();
    observer.error(cause instanceof Error ? cause : new Error('Live history is unavailable.'));
  };
  const reportStatus = (next: HistoryConnection) => {
    if (closed || next === status) return;
    status = next;
    try {
      observer.status(next);
    } catch (error) {
      fail(error);
    }
  };
  const onOpen = () => reportStatus('connected');
  const onDisconnect = () => {
    // HTTP/protocol rejection closes EventSource permanently; only CONNECTING retries.
    if (source?.readyState === 2)
      fail(new Error('Live history connection closed. Reconnect to try again.'));
    else reportStatus('reconnecting');
  };
  const onHistory: EventListener = (event) => {
    if (closed) return;
    try {
      const page = readHistoryPage(event as MessageEvent);
      observer.page(page);
      if (page.complete && page.finished) dispose();
    } catch (error) {
      fail(error);
    }
  };
  const onHistoryError: EventListener = (event) => {
    if (closed) return;
    let message = 'Live history is unavailable. Reload to try again.';
    try {
      const problem: unknown = JSON.parse((event as MessageEvent).data);
      if (isRecord(problem) && typeof problem.message === 'string' && problem.message.trim())
        message = problem.message;
    } catch {
      // The transport's safe fallback also covers a malformed error response.
    }
    fail(new Error(message));
  };

  if (signal?.aborted) return dispose;
  signal?.addEventListener('abort', dispose, { once: true });
  reportStatus('connecting');
  if (closed) return dispose;
  try {
    source = create(url);
    if (closed) {
      source.close();
      return dispose;
    }
    source.addEventListener('open', onOpen);
    source.addEventListener('error', onDisconnect);
    source.addEventListener('history', onHistory);
    source.addEventListener('history_error', onHistoryError);
  } catch (error) {
    fail(error);
  }
  return dispose;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function readHistoryPage(event: MessageEvent): HistoryPage {
  let page: unknown;
  try {
    page = JSON.parse(event.data);
  } catch {
    throw invalidHistory();
  }
  if (
    !isRecord(page) ||
    !Array.isArray(page.events) ||
    page.events.length > 256 ||
    !page.events.every(
      (record: unknown) =>
        isRecord(record) &&
        typeof record.cursor === 'string' &&
        isRecord(record.event) &&
        typeof record.event.kind === 'string'
    ) ||
    typeof page.nextCursor !== 'string' ||
    typeof page.headCursor !== 'string' ||
    typeof page.complete !== 'boolean' ||
    (page.finished !== undefined && typeof page.finished !== 'boolean') ||
    event.lastEventId !== page.nextCursor
  ) {
    throw invalidHistory();
  }
  const result = page as HistoryPage;
  readPageRuntimeFailure(result);
  return result;
}

function invalidHistory(): Error {
  return new Error('Live history contains an invalid page. Reload to try again.');
}
