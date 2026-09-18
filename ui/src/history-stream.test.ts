import test from 'node:test';
import assert from 'node:assert/strict';
import { watchHistoryEvents, type HistoryConnection, type HistoryObserver } from './history-stream';
import { appendHistory } from './run-history-source';
import type { HistoryEvent, HistoryPage } from './run-history';

class FakeEventSource {
  readonly listeners = new Map<string, Set<EventListener>>();
  closed = 0;
  readyState = 0;
  addEventListener(type: string, listener: EventListener) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(listener);
  }
  removeEventListener(type: string, listener: EventListener) {
    this.listeners.get(type)?.delete(listener);
  }
  close() {
    this.closed++;
    this.readyState = 2;
  }
  emit(type: string, data = '', lastEventId = '') {
    if (type === 'open') this.readyState = 1;
    if (type === 'error' && this.readyState !== 2) this.readyState = 0;
    const event = new MessageEvent(type, { data, lastEventId });
    for (const listener of [...(this.listeners.get(type) ?? [])]) listener(event);
  }
  history(page: HistoryPage) {
    this.emit('history', JSON.stringify(page), page.nextCursor);
  }
  get listenerCount() {
    return [...this.listeners.values()].reduce((count, listeners) => count + listeners.size, 0);
  }
}
const page = (values: Partial<HistoryPage> = {}): HistoryPage => ({
  events: [],
  nextCursor: 'v2:0',
  headCursor: 'v2:0',
  complete: true,
  finished: false,
  ...values,
});
function setup(overrides: Partial<HistoryObserver> = {}, signal?: AbortSignal) {
  const source = new FakeEventSource();
  const pages: HistoryPage[] = [];
  const states: HistoryConnection[] = [];
  const errors: Error[] = [];
  let created = 0;
  const dispose = watchHistoryEvents(
    new URL('http://localhost/ui/api/runs/example/events?after=v2%3A0'),
    {
      page: (value) => pages.push(value),
      status: (value) => states.push(value),
      error: (value) => errors.push(value),
      ...overrides,
    },
    signal,
    () => {
      created++;
      return source;
    }
  );
  return {
    source,
    pages,
    states,
    errors,
    dispose,
    get created() {
      return created;
    },
  };
}

test('network reconnect keeps the browser-owned source and resumes delivering pages', () => {
  const stream = setup();
  assert.deepEqual(stream.states, ['connecting']);
  stream.source.emit('open');
  stream.source.history(page());
  stream.source.emit('error');
  stream.source.emit('error');
  stream.source.emit('open');
  stream.source.history(page());
  assert.deepEqual(stream.states, ['connecting', 'connected', 'reconnecting', 'connected']);
  assert.equal(stream.created, 1);
  assert.equal(stream.pages.length, 2);
  assert.equal(stream.errors.length, 0);
  assert.equal(stream.source.closed, 0);
  stream.dispose();
});

test('permanently closed connections report a recoverable error instead of waiting for automatic reconnect', () => {
  for (const connected of [false, true]) {
    const stream = setup();
    if (connected) stream.source.emit('open');
    stream.source.readyState = 2;
    stream.source.emit('error');
    stream.source.emit('error');
    stream.source.history(page());
    stream.dispose();
    assert.equal(stream.source.closed, 1);
    assert.equal(stream.source.listenerCount, 0);
    assert.equal(stream.pages.length, 0);
    assert.deepEqual(stream.states, connected ? ['connecting', 'connected'] : ['connecting']);
    assert.deepEqual(
      stream.errors.map(({ message }) => message),
      ['Live history connection closed. Reconnect to try again.']
    );
  }
});

test('caught-up live pages stay connected and finished history closes only after it is drained', () => {
  const stream = setup();
  stream.source.emit('open');
  stream.source.history(page());
  stream.source.history(page({ headCursor: 'v2:1', complete: false, finished: true }));
  assert.equal(stream.source.closed, 0);
  stream.source.history(page({ finished: true }));
  assert.equal(stream.pages.length, 3);
  assert.equal(stream.source.closed, 1);
  assert.equal(stream.source.listenerCount, 0);
  stream.source.emit('error');
  stream.source.history(page());
  stream.dispose();
  assert.deepEqual(stream.states, ['connecting', 'connected']);
  assert.equal(stream.pages.length, 3);
  assert.equal(stream.source.closed, 1);
});

test('runtime failure drains retained pages and closes without adding a terminal event', () => {
  let retained: HistoryEvent[] = [];
  const stream = setup({
    page: (incoming) => {
      retained = appendHistory(retained, incoming);
    },
  });
  const runtimeFailure = { atCursor: 'v2:2', reason: 'runtime_failed' } as const;
  const records: HistoryEvent[] = [1, 2, 3].map((value) => ({
    cursor: `v2:${value}`,
    event: { kind: 'safe_log', line: `Retained ${value}` },
  }));
  stream.source.history(
    page({
      events: records.slice(0, 1),
      nextCursor: 'v2:1',
      headCursor: 'v2:3',
      complete: false,
      finished: true,
      runtimeFailure,
    })
  );
  assert.equal(stream.source.closed, 0);
  assert.equal(retained.length, 1);
  stream.source.history(
    page({
      events: records.slice(1),
      nextCursor: 'v2:3',
      headCursor: 'v2:3',
      complete: true,
      finished: true,
      runtimeFailure,
    })
  );
  assert.equal(stream.source.closed, 1);
  assert.deepEqual(retained, records);
  assert.equal(stream.errors.length, 0);
});

test('malformed runtime failure metadata cannot reach a live observer', () => {
  for (const runtimeFailure of [
    null,
    {},
    { atCursor: 'v2:0', reason: 'unknown' },
    { atCursor: 'v2:1', reason: 'runtime_failed' },
    { atCursor: 'v2:00', reason: 'runtime_failed' },
  ]) {
    const stream = setup();
    stream.source.emit(
      'history',
      JSON.stringify({ ...page(), finished: true, runtimeFailure }),
      'v2:0'
    );
    assert.equal(stream.pages.length, 0);
    assert.equal(stream.source.closed, 1);
    assert.match(stream.errors[0]?.message ?? '', /inconsistent with retained history/);
  }
});

test('abort and explicit disposal detach every listener and do not surface intentional shutdown', () => {
  for (const abort of [true, false]) {
    const controller = new AbortController();
    const stream = setup({}, controller.signal);
    if (abort) controller.abort();
    else stream.dispose();
    stream.source.emit('open');
    stream.source.emit('history_error', '{"message":"too late"}');
    stream.source.history(page());
    controller.abort();
    stream.dispose();
    assert.equal(stream.source.closed, 1);
    assert.equal(stream.source.listenerCount, 0);
    assert.deepEqual(stream.states, ['connecting']);
    assert.equal(stream.pages.length, 0);
    assert.equal(stream.errors.length, 0);
  }
  const controller = new AbortController();
  controller.abort();
  const stream = setup({}, controller.signal);
  assert.equal(stream.created, 0);
  assert.deepEqual(stream.states, []);
});

test('malformed pages and mismatched resume IDs fail closed before entering the replay', () => {
  for (const [data, id] of [
    ['not json', 'v2:0'],
    ['null', 'v2:0'],
    [JSON.stringify(page({ events: [null] as unknown as HistoryEvent[] })), 'v2:0'],
    [
      JSON.stringify(
        page({ events: [{ cursor: 'v2:1', event: null }] as unknown as HistoryEvent[] })
      ),
      'v2:0',
    ],
    [JSON.stringify({ ...page(), complete: 'yes' }), 'v2:0'],
    [JSON.stringify({ ...page(), finished: 'yes' }), 'v2:0'],
    [JSON.stringify(page()), 'v2:9'],
    [JSON.stringify(page()), ''],
  ]) {
    const stream = setup();
    stream.source.emit('history', data, id);
    assert.equal(stream.pages.length, 0);
    assert.equal(stream.source.closed, 1);
    assert.equal(stream.source.listenerCount, 0);
    assert.match(stream.errors[0]?.message ?? '', /invalid page/);
  }
});

test('a fatal server error closes the stream and surfaces its safe message', () => {
  for (const [data, expected] of [
    [
      '{"code":"SOURCE_UNAVAILABLE","message":"Retained history is unavailable."}',
      'Retained history is unavailable.',
    ],
    ['not json', 'Live history is unavailable. Reload to try again.'],
    ['{"message":null}', 'Live history is unavailable. Reload to try again.'],
  ]) {
    const stream = setup();
    stream.source.emit('history_error', data);
    stream.source.emit('error');
    assert.equal(stream.source.closed, 1);
    assert.equal(stream.source.listenerCount, 0);
    assert.deepEqual(
      stream.errors.map(({ message }) => message),
      [expected]
    );
    assert.deepEqual(stream.states, ['connecting']);
  }
});

test('cursor validation errors from the observer stop reconnection and preserve the valid prefix', () => {
  let events: HistoryEvent[] = [];
  const stream = setup({
    page: (incoming) => {
      events = appendHistory(events, incoming);
    },
  });
  stream.source.history(
    page({
      events: [{ cursor: 'v2:1', event: { kind: 'run_started' } }],
      nextCursor: 'v2:1',
      headCursor: 'v2:1',
    })
  );
  stream.source.history(
    page({
      events: [{ cursor: 'v2:3', event: { kind: 'run_started' } }],
      nextCursor: 'v2:3',
      headCursor: 'v2:3',
    })
  );
  stream.source.emit('error');
  assert.equal(events.length, 1);
  assert.equal(stream.source.closed, 1);
  assert.equal(stream.source.listenerCount, 0);
  assert.match(stream.errors[0]?.message ?? '', /gap or inconsistent cursor/);
});

test('status callback errors and constructor failures close and surface exactly once', () => {
  const stream = setup({
    status: (state) => {
      if (state === 'connected') throw new Error('status failed');
    },
  });
  stream.source.emit('open');
  stream.source.emit('error');
  assert.equal(stream.source.closed, 1);
  assert.deepEqual(
    stream.errors.map(({ message }) => message),
    ['status failed']
  );

  const initial = setup({
    status: () => {
      throw new Error('initial status failed');
    },
  });
  assert.equal(initial.created, 0);
  assert.deepEqual(
    initial.errors.map(({ message }) => message),
    ['initial status failed']
  );

  const errors: Error[] = [];
  const dispose = watchHistoryEvents(
    new URL('http://localhost/ui/api/runs/example/events?after=v2%3A0'),
    { page() {}, status() {}, error: (error) => errors.push(error) },
    undefined,
    () => {
      throw new Error('constructor failed');
    }
  );
  dispose();
  assert.deepEqual(
    errors.map(({ message }) => message),
    ['constructor failed']
  );
});
