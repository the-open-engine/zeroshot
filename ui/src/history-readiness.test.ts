import test from 'node:test';
import assert from 'node:assert/strict';
import { readHistoryWhenReady, waitForHistoryRetry } from './history-readiness';

const pending = new Error('The source is not ready.');
const RETRY_MS = 37;
const retryDelay = (error: unknown) => (error === pending ? RETRY_MS : undefined);

test('pending history retries sequentially until the read succeeds', async () => {
  const controller = new AbortController();
  const delays: number[] = [];
  let reads = 0,
    active = 0,
    maximumActive = 0,
    pendingSignals = 0;
  const value = await readHistoryWhenReady(
    async () => {
      reads++;
      active++;
      maximumActive = Math.max(maximumActive, active);
      try {
        if (reads < 3) throw pending;
        return 'ready';
      } finally {
        active--;
      }
    },
    controller.signal,
    retryDelay,
    () => pendingSignals++,
    async (signal, delayMs) => {
      assert.equal(signal.aborted, false);
      delays.push(delayMs);
    }
  );
  assert.equal(value, 'ready');
  assert.equal(reads, 3);
  assert.equal(maximumActive, 1);
  assert.equal(pendingSignals, 2);
  assert.deepEqual(delays, [RETRY_MS, RETRY_MS]);
});

test('non-pending history errors are returned without polling', async () => {
  const expected = new Error('The source rejected the read.');
  let waits = 0;
  await assert.rejects(
    readHistoryWhenReady(
      async () => {
        throw expected;
      },
      new AbortController().signal,
      retryDelay,
      undefined,
      async () => {
        waits++;
      }
    ),
    (error) => error === expected
  );
  assert.equal(waits, 0);
});

test('aborting a pending read clears its retry and prevents another request', async () => {
  const controller = new AbortController();
  let reads = 0,
    announce!: () => void;
  const announced = new Promise<void>((resolve) => {
    announce = resolve;
  });
  const reading = readHistoryWhenReady(
    async () => {
      reads++;
      throw pending;
    },
    controller.signal,
    retryDelay,
    announce,
    waitForHistoryRetry
  );
  await announced;
  controller.abort();
  await assert.rejects(reading, { name: 'AbortError' });
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(reads, 1);
});
