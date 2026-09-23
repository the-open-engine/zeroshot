export type HistoryRetryDelay = (error: unknown) => number | undefined;
export type HistoryRetryWait = (signal: AbortSignal, delayMs: number) => Promise<void>;

function abortError(signal: AbortSignal): Error {
  return signal.reason instanceof Error
    ? signal.reason
    : new DOMException('The operation was aborted.', 'AbortError');
}

export function waitForHistoryRetry(signal: AbortSignal, delayMs: number): Promise<void> {
  if (signal.aborted) return Promise.reject(abortError(signal));
  return new Promise((resolve, reject) => {
    const finish = () => {
      signal.removeEventListener('abort', abort);
      resolve();
    };
    const abort = () => {
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      reject(abortError(signal));
    };
    const timer = setTimeout(finish, delayMs);
    signal.addEventListener('abort', abort, { once: true });
  });
}

export async function readHistoryWhenReady<T>(
  read: () => Promise<T>,
  signal: AbortSignal,
  retryDelay: HistoryRetryDelay | undefined,
  onRetry: () => void = () => {},
  wait: HistoryRetryWait = waitForHistoryRetry
): Promise<T> {
  while (!signal.aborted) {
    try {
      const value = await read();
      if (signal.aborted) throw abortError(signal);
      return value;
    } catch (error) {
      if (signal.aborted) throw abortError(signal);
      const delayMs = retryDelay?.(error);
      if (delayMs === undefined) throw error;
      onRetry();
      await wait(signal, delayMs);
    }
  }
  throw abortError(signal);
}
