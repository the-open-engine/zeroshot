import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError } from './api';
import { watchHistoryEvents, type HistoryConnection, type HistoryObserver } from './history-stream';
import { appendHistory } from './run-history-source';
import { canFollowHistory } from './history-contract';
import type { HistoryEvent, HistoryPage, RunDetail } from './run-history';

const page = (values: Partial<HistoryPage> = {}): HistoryPage => ({
  events: [],
  nextCursor: 'v2:0',
  headCursor: 'v2:0',
  complete: true,
  finished: false,
  ...values,
});
const frame = (value: HistoryPage, id = value.nextCursor) =>
  `event: history\nid: ${id}\ndata: ${JSON.stringify(value)}\n\n`;
async function until(predicate: () => boolean) {
  for (let attempt = 0; attempt < 100 && !predicate(); attempt++)
    await new Promise((resolve) => setTimeout(resolve, 10));
  assert.ok(predicate(), 'the expected stream transition did not occur');
}
function channel() {
  let output!: ReadableStreamDefaultController<Uint8Array>;
  let cancelled = false;
  const response = new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        output = controller;
      },
      cancel() {
        cancelled = true;
      },
    }),
    { headers: { 'Content-Type': 'text/event-stream' } }
  );
  return {
    response,
    send(value: string) {
      output.enqueue(new TextEncoder().encode(value));
    },
    disconnect() {
      output.error(new Error('connection lost'));
    },
    close() {
      output.close();
    },
    get cancelled() {
      return cancelled;
    },
  };
}
function setup(
  fetcher: typeof fetch,
  overrides: Partial<HistoryObserver> = {},
  signal?: AbortSignal
) {
  const pages: HistoryPage[] = [],
    states: HistoryConnection[] = [],
    errors: Error[] = [];
  const dispose = watchHistoryEvents(
    new URL('https://example.test/ui/api/runs/run/events?after=v2%3A0'),
    {
      page: (value) => pages.push(value),
      status: (value) => states.push(value),
      error: (value) => errors.push(value),
      ...overrides,
    },
    signal,
    fetcher
  );
  return { pages, states, errors, dispose };
}

test('network read failure resumes after the last accepted execution cursor', async (t) => {
  const first = channel(),
    second = channel();
  const requests: RequestInit[] = [];
  const stream = setup(async (_url, init) => {
    requests.push(init!);
    return requests.length === 1 ? first.response : second.response;
  });
  t.after(stream.dispose);
  await until(() => stream.states.includes('connected'));
  first.send(
    frame(
      page({
        events: [{ cursor: 'v2:1', event: { kind: 'run_started' } }],
        nextCursor: 'v2:1',
        headCursor: 'v2:1',
      })
    )
  );
  await until(() => stream.pages.length === 1);
  first.disconnect();
  await until(() => requests.length === 2);
  assert.equal(new Headers(requests[1].headers).get('Last-Event-ID'), 'v2:1');
  assert.deepEqual(stream.states, ['connecting', 'connected', 'reconnecting', 'connected']);
  assert.equal(stream.errors.length, 0);
});
test('HTTP denials retain status, problem code and details without reconnecting', async () => {
  for (const status of [401, 403, 410, 503]) {
    let calls = 0;
    const stream = setup(async () => {
      calls++;
      return Response.json(
        { code: `problem_${status}`, message: 'History access ended.', details: { status } },
        { status }
      );
    });
    await until(() => stream.errors.length === 1);
    const error = stream.errors[0];
    assert.ok(error instanceof ApiError);
    assert.equal(error.status, status);
    assert.equal(error.code, `problem_${status}`);
    assert.deepEqual(error.details, { status });
    assert.equal(calls, 1);
    stream.dispose();
  }
});
test('collecting after runtime failure stays open and completion drains before closing', async (t) => {
  const source = channel();
  const stream = setup(async () => source.response);
  t.after(stream.dispose);
  source.send(
    frame(
      page({
        finished: true,
        runtimeFailure: { atCursor: 'v2:0', reason: 'runtime_failed' },
        observation: { state: 'collecting' },
      })
    )
  );
  await until(() => stream.pages.length === 1);
  assert.equal(source.cancelled, false);
  source.send(
    frame(
      page({
        complete: false,
        headCursor: 'v2:1',
        finished: true,
        observation: { state: 'complete' },
      })
    )
  );
  await until(() => stream.pages.length === 2);
  assert.equal(source.cancelled, false);
  source.send(frame(page({ finished: true, observation: { state: 'complete' } })));
  await until(() => source.cancelled);
  assert.equal(stream.pages.length, 3);
  assert.equal(stream.errors.length, 0);
});
test('SSE framing handles split CRLF and bounds incomplete frames', async (t) => {
  const source = channel();
  const stream = setup(async () => source.response);
  t.after(stream.dispose);
  for (const character of frame(page()).replaceAll('\n', '\r\n')) source.send(character);
  await until(() => stream.pages.length === 1);
  source.send('data: ' + 'x'.repeat(8 * 1024 * 1024));
  await until(() => stream.errors.length === 1);
  assert.match(stream.errors[0].message, /8 MiB/);
  assert.equal(source.cancelled, true);
});
test('structured stream errors preserve problem codes and stop follow', async () => {
  const source = channel();
  const stream = setup(async () => source.response);
  source.send(
    'event: history_error\ndata: {"code":"archive_expired","message":"Archive expired.","details":{"days":30}}\n\n'
  );
  await until(() => stream.errors.length === 1);
  assert.ok(stream.errors[0] instanceof ApiError);
  assert.equal(stream.errors[0].code, 'archive_expired');
  assert.deepEqual(stream.errors[0].details, { days: 30 });
  assert.equal(source.cancelled, true);
});
test('observer cursor gaps retain the valid prefix; malformed pages cannot enter replay', async () => {
  const source = channel();
  let retained: HistoryEvent[] = [];
  const stream = setup(async () => source.response, {
    page: (incoming) => {
      retained = appendHistory(retained, incoming);
    },
  });
  source.send(
    frame(
      page({
        events: [{ cursor: 'v2:1', event: { kind: 'run_started' } }],
        nextCursor: 'v2:1',
        headCursor: 'v2:1',
      })
    )
  );
  source.send(
    frame(
      page({
        events: [{ cursor: 'v2:3', event: { kind: 'run_started' } }],
        nextCursor: 'v2:3',
        headCursor: 'v2:3',
      })
    )
  );
  await until(() => stream.errors.length === 1);
  assert.equal(retained.length, 1);
  assert.match(stream.errors[0].message, /gap or inconsistent cursor/);
  for (const input of ['event: history\ndata: null\n\n', frame(page(), 'v2:8')]) {
    const invalid = channel();
    const rejected = setup(async () => invalid.response);
    invalid.send(input);
    await until(() => rejected.errors.length === 1);
    assert.equal(rejected.pages.length, 0);
    assert.ok(rejected.errors[0] instanceof ApiError);
  }
});
test('abort including during a page callback cancels readers without false errors', async () => {
  for (const duringPage of [false, true]) {
    const source = channel(),
      controller = new AbortController();
    const stream = setup(
      async () => source.response,
      duringPage ? { page: () => controller.abort() } : {},
      controller.signal
    );
    await until(() => stream.states.includes('connected'));
    if (duringPage) source.send(frame(page()));
    else controller.abort();
    await until(() => source.cancelled);
    assert.equal(stream.errors.length, 0);
    stream.dispose();
  }
  const controller = new AbortController();
  controller.abort();
  let called = false;
  setup(
    async () => {
      called = true;
      throw new Error('must not fetch');
    },
    {},
    controller.signal
  );
  assert.equal(called, false);
});
test('incomplete EOF reconnects without advancing the resume cursor', async (t) => {
  const source = channel(),
    resumed = channel();
  let calls = 0,
    lastId: string | null = null;
  const stream = setup(async (_url, init) => {
    calls++;
    lastId = new Headers(init?.headers).get('Last-Event-ID');
    return calls === 1 ? source.response : resumed.response;
  });
  t.after(stream.dispose);
  source.send('event: history\nid: v2:7\ndata: {');
  source.close();
  await until(() => calls === 2);
  assert.equal(lastId, 'v2:0');
  assert.equal(stream.pages.length, 0);
});
test('terminal run status does not end collecting observation', () => {
  const run = {
    phase: 'finished',
    terminal: { status: 'failed', reason: 'runtime_failed' },
    history: { complete: false },
    observation: { state: 'collecting' },
  } as RunDetail;
  assert.equal(canFollowHistory(run), true);
  for (const state of ['complete', 'incomplete', 'expired', 'unavailable'] as const)
    assert.equal(canFollowHistory({ ...run, observation: { state } }), false);
});
