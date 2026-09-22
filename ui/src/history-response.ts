import { ApiError } from './api';
import type { HistoryPage } from './run-history';
import { readObservation, readPageRuntimeFailure } from './history-contract';

const MAX_FRAME_BYTES = 8 * 1024 * 1024;
const MAX_PROBLEM_BYTES = 64 * 1024;
const encoder = new TextEncoder();

export const HISTORY_PENDING_RETRY_MS = 3_000;

/** The HTTP adapter owns the server problem codes that are safe to retry. */
export function historyRetryDelay(error: unknown): number | undefined {
  return error instanceof ApiError && error.code === 'history_pending'
    ? HISTORY_PENDING_RETRY_MS
    : undefined;
}

/** Only complete SSE history records can advance the caller's resume cursor. */
export async function readHistoryResponse(
  response: Response,
  signal: AbortSignal,
  page: (value: HistoryPage) => void
): Promise<void> {
  if (
    response.headers.get('content-type')?.split(';')[0].trim() !== 'text/event-stream' ||
    !response.body
  )
    throw new ApiError(
      response.status,
      'invalid_history_stream',
      'The server did not return a history stream.'
    );
  const reader = response.body.getReader();
  const abort = () => {
    void reader.cancel().catch(() => {});
  };
  signal.addEventListener('abort', abort, { once: true });
  const frames = new HistoryFrames(page);
  const decoder = new TextDecoder('utf-8', { fatal: true });
  try {
    while (!signal.aborted) {
      const part = await reader.read().catch(() => undefined);
      if (!part || part.done || signal.aborted) return;
      frames.push(decoder.decode(part.value, { stream: true }), signal);
    }
  } finally {
    signal.removeEventListener('abort', abort);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

class HistoryFrames {
  private pending = '';
  private event = '';
  private id = '';
  private data: string[] = [];
  private bytes = 0;
  constructor(private readonly page: (value: HistoryPage) => void) {}
  push(text: string, signal: AbortSignal) {
    this.pending += text;
    let newline: number;
    while (!signal.aborted && (newline = this.pending.search(/[\r\n]/)) !== -1) {
      if (this.pending[newline] === '\r' && newline === this.pending.length - 1) break;
      const line = this.pending.slice(0, newline);
      const width = this.pending.slice(newline, newline + 2) === '\r\n' ? 2 : 1;
      this.pending = this.pending.slice(newline + width);
      this.bytes += encoder.encode(line).byteLength;
      this.checkSize(0);
      this.line(line);
    }
    this.checkSize(encoder.encode(this.pending).byteLength);
  }
  private checkSize(pending: number) {
    if (pending + this.bytes > MAX_FRAME_BYTES)
      throw invalidHistory('A history frame exceeds the 8 MiB browser limit.');
  }
  private line(line: string) {
    if (!line) return this.dispatch();
    if (line.startsWith(':')) return;
    const separator = line.indexOf(':');
    const field = separator < 0 ? line : line.slice(0, separator);
    const value = separator < 0 ? '' : line.slice(separator + 1).replace(/^ /, '');
    if (field === 'event') this.event = value;
    else if (field === 'id') this.id = value;
    else if (field === 'data') this.data.push(value);
  }
  private dispatch() {
    const value = this.data.join('\n');
    if (this.event === 'history') this.page(readHistoryPage(value, this.id));
    else if (this.event === 'history_error') throw historyProblem(parseJson(value), 200);
    this.event = '';
    this.id = '';
    this.data = [];
    this.bytes = 0;
  }
}
export async function readHistoryProblem(
  response: Response,
  signal: AbortSignal
): Promise<ApiError> {
  const reader = response.body?.getReader();
  if (!reader) return historyProblem(undefined, response.status);
  const abort = () => {
    void reader.cancel().catch(() => {});
  };
  signal.addEventListener('abort', abort, { once: true });
  let length = 0;
  let text = '';
  const decoder = new TextDecoder();
  try {
    while (!signal.aborted) {
      const part = await reader.read();
      if (part.done) break;
      length += part.value.byteLength;
      if (length > MAX_PROBLEM_BYTES) return historyProblem(undefined, response.status);
      text += decoder.decode(part.value, { stream: true });
    }
    return historyProblem(parseJson(text + decoder.decode()), response.status);
  } finally {
    signal.removeEventListener('abort', abort);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}
function historyProblem(value: unknown, status: number): ApiError {
  const problem = isRecord(value) ? value : {};
  return new ApiError(
    status,
    typeof problem.code === 'string' ? problem.code : 'history_unavailable',
    typeof problem.message === 'string' && problem.message.trim()
      ? problem.message
      : 'Live history is unavailable. Reload to try again.',
    problem.details
  );
}
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function isEvent(value: unknown): boolean {
  return (
    isRecord(value) &&
    typeof value.cursor === 'string' &&
    isRecord(value.event) &&
    typeof value.event.kind === 'string'
  );
}
function readHistoryPage(data: string, id: string): HistoryPage {
  const page = parseJson(data);
  if (
    !isRecord(page) ||
    !Array.isArray(page.events) ||
    page.events.length > 256 ||
    !page.events.every(isEvent)
  )
    throw invalidHistory();
  if (
    typeof page.nextCursor !== 'string' ||
    typeof page.headCursor !== 'string' ||
    typeof page.complete !== 'boolean' ||
    id !== page.nextCursor ||
    (page.finished !== undefined && typeof page.finished !== 'boolean')
  )
    throw invalidHistory();
  const result = page as HistoryPage;
  readPageRuntimeFailure(result);
  readObservation(result.observation);
  return result;
}
function invalidHistory(
  message = 'Live history contains an invalid page. Reload to try again.'
): ApiError {
  return new ApiError(200, 'invalid_history', message);
}
