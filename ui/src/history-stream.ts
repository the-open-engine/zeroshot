import type { HistoryPage } from './run-history';
import { observationEnded } from './history-contract';
import { readHistoryProblem, readHistoryResponse } from './history-response';

export type HistoryConnection = 'connecting' | 'connected' | 'reconnecting';
export type HistoryObserver = {
  page(page: HistoryPage): void;
  status(state: HistoryConnection): void;
  error(error: Error): void;
};

/** Each follow owns one reader, reconnect timer and last successfully accepted cursor. */
export function watchHistoryEvents(
  url: URL,
  observer: HistoryObserver,
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch
): () => void {
  const watch = new HistoryWatch(url, observer, signal, fetcher);
  watch.start();
  return watch.dispose;
}
class HistoryWatch {
  private readonly controller = new AbortController();
  private cursor: string;
  private status?: HistoryConnection;
  private timer?: ReturnType<typeof setTimeout>;
  private wake?: () => void;
  constructor(
    private readonly url: URL,
    private readonly observer: HistoryObserver,
    private readonly signal: AbortSignal | undefined,
    private readonly fetcher: typeof fetch
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
    if (this.timer) clearTimeout(this.timer);
    this.wake?.();
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
  private async reconnect() {
    if (this.controller.signal.aborted) return;
    this.report('reconnecting');
    if (this.controller.signal.aborted) return;
    await new Promise<void>((resolve) => {
      this.wake = resolve;
      this.timer = setTimeout(resolve, 500);
    });
    this.timer = undefined;
    this.wake = undefined;
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
  private async connect() {
    const signal = this.controller.signal;
    while (!signal.aborted) {
      const response = await this.request();
      if (signal.aborted) {
        await response?.body?.cancel().catch(() => {});
        return;
      }
      if (!response) {
        await this.reconnect();
        continue;
      }
      if (!response.ok) throw await readHistoryProblem(response, signal);
      this.report('connected');
      if (signal.aborted) {
        await response.body?.cancel().catch(() => {});
        return;
      }
      await readHistoryResponse(response, signal, this.page);
      await this.reconnect();
    }
  }
}
