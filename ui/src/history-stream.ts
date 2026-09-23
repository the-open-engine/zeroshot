import type { HistoryPage } from './run-history';
import { observationEnded } from './history-contract';
import {
  historyRetryDelay,
  readHistoryProblem,
  readHistoryResponse,
} from './history-response';
import { waitForHistoryRetry, type HistoryRetryWait } from './history-readiness';

export type HistoryConnection = 'connecting' | 'connected' | 'reconnecting';
export type HistoryObserver = {
  page(page: HistoryPage): void;
  status(state: HistoryConnection): void;
  error(error: Error): void;
};
async function cancelResponse(response?: Response) {
  await response?.body?.cancel().catch(() => {});
}

/** Each follow owns one reader, reconnect timer and last successfully accepted cursor. */
export function watchHistoryEvents(
  url: URL,
  observer: HistoryObserver,
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch,
  wait: HistoryRetryWait = waitForHistoryRetry
): () => void {
  const watch = new HistoryWatch(url, observer, signal, fetcher, wait);
  watch.start();
  return watch.dispose;
}
class HistoryWatch {
  private readonly controller = new AbortController();
  private cursor: string;
  private status?: HistoryConnection;
  constructor(
    private readonly url: URL,
    private readonly observer: HistoryObserver,
    private readonly signal: AbortSignal | undefined,
    private readonly fetcher: typeof fetch,
    private readonly wait: HistoryRetryWait
  ) {
    this.cursor = url.searchParams.get('after') ?? 'v2:0';
  }
  start() {
    if (this.signal?.aborted) return this.dispose();
    this.signal?.addEventListener('abort', this.dispose, { once: true });
    this.report('connecting');
    void this.connect().catch((cause) => this.fail(cause));
  }
  readonly dispose = () => {
    if (this.controller.signal.aborted) return;
    this.controller.abort();
    this.signal?.removeEventListener('abort', this.dispose);
  };
  private fail(cause: unknown) {
    if (this.controller.signal.aborted) return;
    this.dispose();
    this.observer.error(cause instanceof Error ? cause : new Error('History is unavailable.'));
  }
  private report(next: HistoryConnection) {
    if (this.controller.signal.aborted || this.status === next) return;
    this.status = next;
    try {
      this.observer.status(next);
    } catch (cause) {
      this.fail(cause);
    }
  }
  private async reconnect(delayMs = 500) {
    if (this.controller.signal.aborted) return;
    this.report('reconnecting');
    await this.wait(this.controller.signal, delayMs);
  }
  private async request(): Promise<Response | undefined> {
    try {
      return await this.fetcher(this.url, {
        headers: { Accept: 'text/event-stream', 'Last-Event-ID': this.cursor },
        credentials: 'same-origin',
        cache: 'no-store',
        redirect: 'error',
        signal: this.controller.signal,
      });
    } catch {
      return undefined;
    }
  }
  private readonly page = (page: HistoryPage) => {
    this.observer.page(page);
    this.cursor = page.nextCursor;
    if (page.complete && observationEnded(page.observation, page.finished === true)) this.dispose();
  };
  private async read(response: Response) {
    const signal = this.controller.signal;
    if (!response.ok) throw await readHistoryProblem(response, signal);
    this.report('connected');
    if (signal.aborted) return cancelResponse(response);
    await readHistoryResponse(response, signal, this.page);
  }
  private async connect() {
    const signal = this.controller.signal;
    while (!signal.aborted) {
      const response = await this.request();
      if (signal.aborted) return cancelResponse(response);
      if (!response) {
        await this.reconnect();
        continue;
      }
      try {
        await this.read(response);
      } catch (cause) {
        if (signal.aborted) return;
        const delayMs = historyRetryDelay(cause);
        if (delayMs === undefined) throw cause;
        await this.reconnect(delayMs);
        continue;
      }
      await this.reconnect();
    }
  }
}
