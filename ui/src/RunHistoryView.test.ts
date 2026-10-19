import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement } from 'react';
import type { Root } from 'react-dom/client';
import { renderToStaticMarkup } from 'react-dom/server';
import { JSDOM } from 'jsdom';
import type { HistoryPage, RunDetail } from './run-history';
import { ApiError } from './api';
import type { RunHistoryReader } from './run-history-source';
import { createViteTestServer, runDetailFixture } from './test-support';

function stubWorker(t: TestContext) {
  const worker = Object.getOwnPropertyDescriptor(globalThis, 'Worker');
  Object.defineProperty(globalThis, 'Worker', {
    configurable: true,
    value: class {
      postMessage() {}
      terminate() {}
    },
  });
  t.after(() => {
    if (worker) Object.defineProperty(globalThis, 'Worker', worker);
    else Reflect.deleteProperty(globalThis, 'Worker');
  });
}

async function viewerModule(t: TestContext) {
  const server = await createViteTestServer(t);
  const viewer = await server.ssrLoadModule('/src/RunHistoryView.tsx');
  return { RunHistoryView: viewer.RunHistoryView };
}

const completePage = (): HistoryPage => ({
  events: [],
  nextCursor: 'v2:0',
  headCursor: 'v2:0',
  complete: true,
});

const renderedText = (container: HTMLElement) => container.textContent ?? '';
async function clickButton(container: HTMLElement, label: string) {
  const button = container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`);
  assert.ok(button, label);
  await act(async () => button.click());
}
async function loadMoreHistory(container: HTMLElement) {
  const button = [...container.querySelectorAll('button')].find(
    (candidate) => candidate.textContent === 'Load more history'
  );
  assert.ok(button);
  await act(async () => button.click());
}
const browserGlobals = [
  'window',
  'document',
  'navigator',
  'Element',
  'HTMLElement',
  'SVGElement',
  'Node',
  'requestAnimationFrame',
  'cancelAnimationFrame',
  'getComputedStyle',
  'ResizeObserver',
] as const;

function installBrowser(t: TestContext) {
  const dom = new JSDOM('<!doctype html><html><body></body></html>', {
    pretendToBeVisual: true,
    url: 'https://example.test/',
  });
  const descriptors = new Map(
    browserGlobals.map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)])
  );
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  const globals = {
    window: dom.window,
    document: dom.window.document,
    navigator: dom.window.navigator,
    Element: dom.window.Element,
    HTMLElement: dom.window.HTMLElement,
    SVGElement: dom.window.SVGElement,
    Node: dom.window.Node,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    getComputedStyle: dom.window.getComputedStyle.bind(dom.window),
    ResizeObserver: ResizeObserverStub,
  };
  for (const name of browserGlobals)
    Object.defineProperty(globalThis, name, { configurable: true, value: globals[name] });
  Object.defineProperty(dom.window, 'ResizeObserver', {
    configurable: true,
    value: ResizeObserverStub,
  });
  Object.defineProperties(dom.window.HTMLElement.prototype, {
    clientWidth: { configurable: true, get: () => 800 },
    clientHeight: { configurable: true, get: () => 600 },
    offsetWidth: { configurable: true, get: () => 800 },
    offsetHeight: { configurable: true, get: () => 600 },
  });
  dom.window.HTMLElement.prototype.getBoundingClientRect = () => ({
    x: 0,
    y: 0,
    top: 0,
    right: 800,
    bottom: 600,
    left: 0,
    width: 800,
    height: 600,
    toJSON: () => ({}),
  });
  const styles = dom.window.document.createElement('style');
  styles.textContent = '.react-flow__pane { z-index: 1; }';
  dom.window.document.head.append(styles);
  const actGlobal = globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean };
  const actEnvironment = actGlobal.IS_REACT_ACT_ENVIRONMENT;
  actGlobal.IS_REACT_ACT_ENVIRONMENT = true;
  t.after(() => {
    actGlobal.IS_REACT_ACT_ENVIRONMENT = actEnvironment;
    for (const name of browserGlobals) {
      const descriptor = descriptors.get(name);
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else Reflect.deleteProperty(globalThis, name);
    }
    dom.window.close();
  });
  return dom.window;
}

async function flushEffects() {
  await act(async () => {
    await new Promise<void>((resolve) => setImmediate(resolve));
  });
}

function installHistoryRetryTimers(t: TestContext) {
  const originalSetTimeout = globalThis.setTimeout;
  const originalClearTimeout = globalThis.clearTimeout;
  const retryTimers = new Map<ReturnType<typeof setTimeout>, () => void>();
  let timerId = 0;
  globalThis.setTimeout = ((
    callback: (...args: unknown[]) => void,
    delay?: number,
    ...args: unknown[]
  ) => {
    if (delay === 3_000) {
      const token = { retryTimer: ++timerId } as unknown as ReturnType<typeof setTimeout>;
      retryTimers.set(token, () => callback(...args));
      return token;
    }
    return originalSetTimeout(callback, delay, ...args);
  }) as typeof setTimeout;
  globalThis.clearTimeout = ((timer?: ReturnType<typeof setTimeout>) => {
    if (timer && retryTimers.delete(timer)) return;
    originalClearTimeout(timer);
  }) as typeof clearTimeout;
  t.after(() => {
    globalThis.setTimeout = originalSetTimeout;
    globalThis.clearTimeout = originalClearTimeout;
  });
  return {
    retryTimers,
    async advanceRetry() {
      const entry = retryTimers.entries().next().value;
      assert.ok(entry, 'expected a pending history retry timer');
      const [token, callback] = entry;
      retryTimers.delete(token);
      await act(async () => callback());
      await flushEffects();
    },
  };
}

test('a host can render the viewer without local navigation, a run catalog, or an HTTP adapter', async (t) => {
  // ELK's browser worker is idle until a graph is selected. No layout belongs to this shell test.
  stubWorker(t);
  const { RunHistoryView } = await viewerModule(t);
  const source: RunHistoryReader = {
    detail: async () => {
      throw new Error('Only a selected run may load history.');
    },
    page: async () => {
      throw new Error('Only a selected run may load history.');
    },
  };
  const html = renderToStaticMarkup(createElement(RunHistoryView, { source, workers: [] }));
  assert.match(html, /Run history/);
  assert.doesNotMatch(html, /app-header|run-sidebar|LOCAL|TARGET/);
});

test('the mounted graph view retries pending detail and page reads and aborts on unmount', async (t) => {
  stubWorker(t);
  installBrowser(t);
  const { retryTimers, advanceRetry } = installHistoryRetryTimers(t);

  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const pending = new Error('The source is not ready.');
  const retryDelay = (error: unknown) => (error === pending ? 3_000 : undefined);
  let detailReads = 0,
    pageReads = 0;
  const source: RunHistoryReader = {
    retryDelay,
    async detail() {
      if (++detailReads === 1) throw pending;
      return runDetailFixture('pending-run');
    },
    async page() {
      if (++pageReads === 1) throw pending;
      return completePage();
    },
  };
  const container = document.createElement('div');
  document.body.append(container);
  let root: Root = createRoot(container);
  await act(async () => {
    root.render(createElement(RunHistoryView, { runId: 'pending-run', source, workers: [] }));
  });
  await flushEffects();
  assert.match(renderedText(container), /Run view is still setting up/);
  assert.match(renderedText(container), /The graph will appear here automatically when it’s ready/);

  await advanceRetry();
  assert.equal(detailReads, 2);
  assert.equal(pageReads, 1);
  assert.match(renderedText(container), /Run history is still setting up/);

  await advanceRetry();
  assert.equal(pageReads, 2);
  assert.doesNotMatch(renderedText(container), /still setting up/);
  assert.match(renderedText(container), /Review a change/);
  await act(async () => root.unmount());

  let abortedSignal: AbortSignal | undefined,
    readsAfterUnmount = 0;
  const neverReady: RunHistoryReader = {
    retryDelay,
    async detail(_id, signal) {
      readsAfterUnmount++;
      abortedSignal = signal;
      throw pending;
    },
    async page() {
      throw new Error('A pending definition must not request a page.');
    },
  };
  root = createRoot(container);
  await act(async () =>
    root.render(
      createElement(RunHistoryView, { runId: 'pending-run', source: neverReady, workers: [] })
    )
  );
  await flushEffects();
  assert.equal(retryTimers.size, 1);
  assert.equal(abortedSignal?.aborted, false);
  await act(async () => root.unmount());
  assert.equal(abortedSignal?.aborted, true);
  assert.equal(retryTimers.size, 0);
  await flushEffects();
  assert.equal(readsAfterUnmount, 1);
  container.remove();
});

test('preparation logs are visible before the first node and remain readable after setup failure', async (t) => {
  stubWorker(t);
  installBrowser(t);
  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const detail = { ...runDetailFixture('preparing-run'), phase: 'admitted' };
  const events: HistoryPage['events'] = [
    {
      cursor: 'v2:1',
      event: {
        kind: 'safe_log',
        execution: null,
        stream: 'system',
        line: 'Installing the project toolchain',
      },
    },
  ];
  const source: RunHistoryReader = {
    detail: async () => detail,
    page: async () => ({
      events,
      nextCursor: `v2:${events.length}`,
      headCursor: `v2:${events.length}`,
      complete: true,
    }),
  };
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () =>
      root.render(
        createElement(RunHistoryView, {
          runId: detail.runId,
          source,
          workers: [],
        })
      )
    );
    await flushEffects();
    assert.match(renderedText(container), /Preparing environment/);
    assert.match(renderedText(container), /Installing the project toolchain/);
    assert.ok(container.querySelector('[aria-label="Run log entries"]'));

    const runningSource: RunHistoryReader = {
      ...source,
      page: async () => ({
        events: [...events, { cursor: 'v2:2', event: { kind: 'run_started' } }],
        nextCursor: 'v2:2',
        headCursor: 'v2:2',
        complete: true,
      }),
    };
    await act(async () =>
      root.render(
        createElement(RunHistoryView, {
          key: 'running',
          runId: detail.runId,
          source: runningSource,
          workers: [],
        })
      )
    );
    await flushEffects();
    assert.doesNotMatch(renderedText(container), /Preparing environment/);

    const failed = { status: 'failed' as const, reason: 'environment_setup_failed' };
    events.push({ cursor: 'v2:2', event: { kind: 'terminal', result: failed } });
    const failedSource: RunHistoryReader = {
      ...source,
      detail: async () => ({ ...detail, phase: 'finished', terminal: failed }),
    };
    await act(async () =>
      root.render(
        createElement(RunHistoryView, {
          key: 'failed',
          runId: detail.runId,
          source: failedSource,
          workers: [],
        })
      )
    );
    await flushEffects();
    assert.match(renderedText(container), /Environment setup failed/);
    assert.match(renderedText(container), /Graph execution did not start/);
    assert.match(renderedText(container), /Installing the project toolchain/);
    assert.doesNotMatch(renderedText(container), /History incomplete/);
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});

test('a remounted run restores its paused cursor, inspector and execution without retaining a subscription', async (t) => {
  stubWorker(t);
  const browser = installBrowser(t);
  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const detail = runDetailFixture('remembered-run');
  const events: HistoryPage['events'] = [
    { cursor: 'v2:1', event: { kind: 'run_started' } },
    {
      cursor: 'v2:2',
      event: { kind: 'safe_log', execution: null, stream: 'system', line: 'Fixture log' },
    },
  ];
  let watches = 0;
  let lastSignal: AbortSignal | undefined;
  const source: RunHistoryReader = {
    detail: async () => detail,
    page: async () => ({ ...completePage(), events, nextCursor: 'v2:2', headCursor: 'v2:2' }),
    watch(_id, _after, _observer, signal) {
      watches++;
      lastSignal = signal;
      return () => {
        watches--;
      };
    },
  };
  const container = document.createElement('div');
  document.body.append(container);
  let root = createRoot(container);
  const render = () =>
    act(async () =>
      root.render(
        createElement(RunHistoryView, {
          runId: detail.runId,
          source,
          workers: [],
          viewKey: 'workspace-a:history:remembered-run',
        })
      )
    );
  const click = (label: string) => clickButton(container, label);
  await render();
  await flushEffects();
  assert.equal(watches, 1);
  await click('Go to start');
  await click('Open inspector');
  await click('Next event');
  const position = () =>
    container.querySelector<HTMLInputElement>('[aria-label="History position"]')!.value;
  assert.equal(position(), '1');
  await act(async () => root.unmount());
  assert.equal(watches, 0);
  assert.equal(lastSignal?.aborted, true);
  const saved = JSON.parse(browser.sessionStorage.getItem('workspace-a:history:remembered-run')!);
  assert.equal(saved.cursor, 'v2:1');
  assert.equal(saved.following, false);
  assert.equal(saved.inspector, true);
  root = createRoot(container);
  try {
    await render();
    await flushEffects();
    assert.equal(position(), '1');
    assert.ok(container.querySelector('[aria-label="Run inspector"]'));
    assert.equal(watches, 1, 'only the remounted viewer owns a subscription');
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});

test('bounded replay retains a pending saved cursor and execution until explicit continuation reaches it', async (t) => {
  stubWorker(t);
  const browser = installBrowser(t);
  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const key = 'long-run-view';
  browser.sessionStorage.setItem(
    key,
    JSON.stringify({
      selected: 'run',
      inspector: true,
      execution: 'pinned-execution',
      cursor: 'v2:6000',
      following: false,
    })
  );
  const events: HistoryPage['events'] = Array.from({ length: 6000 }, (_, index) => ({
    cursor: `v2:${index + 1}`,
    event: { kind: 'safe_log', execution: null, stream: 'system', line: 'Fixture output' },
  }));
  let pageReads = 0;
  const source: RunHistoryReader = {
    detail: async () => ({ ...runDetailFixture('long-run'), phase: 'finished' }),
    async page(_id, after) {
      pageReads++;
      const offset = Number(after.slice(3));
      const batch = events.slice(offset, offset + 1000);
      return {
        events: batch,
        nextCursor: batch.at(-1)!.cursor,
        headCursor: 'v2:6000',
        complete: offset + batch.length === events.length,
      };
    },
  };
  const assertPinned = () => {
    const saved = JSON.parse(browser.sessionStorage.getItem(key)!);
    assert.equal(saved.cursor, 'v2:6000');
    assert.equal(saved.execution, 'pinned-execution');
  };
  const container = document.createElement('div');
  document.body.append(container);
  let root = createRoot(container);
  try {
    await act(async () =>
      root.render(
        createElement(RunHistoryView, { runId: 'long-run', source, workers: [], viewKey: key })
      )
    );
    await flushEffects();
    assert.equal(pageReads, 5, 'the initial bounded batch must still stop');
    assert.match(renderedText(container), /Load more history to restore your saved position/);
    assertPinned();
    await loadMoreHistory(container);
    await flushEffects();
    assert.equal(pageReads, 6);
    assert.doesNotMatch(renderedText(container), /restore your saved position/);
    assertPinned();
    await act(async () => root.unmount());
    const saved = JSON.parse(browser.sessionStorage.getItem(key)!);
    browser.sessionStorage.setItem(key, JSON.stringify({ ...saved, cursor: 'v2:1' }));
    root = createRoot(container);
    await act(async () =>
      root.render(
        createElement(RunHistoryView, { runId: 'long-run', source, workers: [], viewKey: key })
      )
    );
    await flushEffects();
    assert.equal(pageReads, 11, 'restoration still reads the full bounded batch');
    assert.equal(
      JSON.parse(browser.sessionStorage.getItem(key)!).cursor,
      'v2:1',
      'pages after the restored position cannot advance it'
    );
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});

function failureHistoryFixture(durable: boolean) {
  const reference = { runId: 'failed-run', node: 'deliver', execution: '1', nodeInstance: '1' };
  const terminal = {
    status: 'failed' as const,
    reason: durable ? 'delivery_failed' : 'runtime_failed',
    context: {
      primary: durable ? undefined : { message: 'Runtime output persistence failed' },
      recorded: [
        {
          message: 'GitHub API request failed: HTTP 404 — base branch not found',
          execution: { execution: 'opaque:execution:1', node: 'deliver' },
          cursor: 'v2:3',
        },
      ],
      incomplete: true,
    },
  };
  const detail = {
    ...runDetailFixture('failed-run'),
    phase: 'finished',
    terminal,
    ...(durable
      ? {}
      : {
          runtimeFailure: {
            atCursor: 'v2:3',
            reason: 'runtime_failed' as const,
            context: terminal.context,
          },
        }),
  };
  detail.graph.root = { kind: 'seq', name: 'run', children: [{ kind: 'step', name: 'deliver' }] };
  const events: HistoryPage['events'] = [
    { cursor: 'v2:1', event: { kind: 'run_started' } },
    { cursor: 'v2:2', event: { kind: 'node_started', reference, input: null, attempt: 1 } },
    {
      cursor: 'v2:3',
      event: {
        kind: 'safe_log',
        execution: '1',
        stream: 'error',
        line: 'Recorded delivery detail',
      },
    },
    ...(durable
      ? [
          {
            cursor: 'v2:4',
            event: {
              kind: 'node_completed',
              completion: { reference, outcome: { status: 'error', code: 'crash' } },
            },
          },
          { cursor: 'v2:5', event: { kind: 'terminal', result: terminal } },
        ]
      : []),
  ];
  const source: RunHistoryReader = {
    detail: async () => detail,
    page: async () => ({
      ...completePage(),
      events,
      nextCursor: durable ? 'v2:5' : 'v2:3',
      headCursor: durable ? 'v2:5' : 'v2:3',
      finished: true,
      ...(!durable ? { runtimeFailure: detail.runtimeFailure } : {}),
    }),
  };
  return { detail, source };
}

for (const durable of [true, false])
  test(`failure context follows replay and log selection (${durable ? 'durable' : 'unpersisted'})`, async (t) => {
    stubWorker(t);
    installBrowser(t);
    const { RunHistoryView } = await viewerModule(t);
    const { createRoot } = await import('react-dom/client');
    const { detail, source } = failureHistoryFixture(durable);
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    const click = (label: string) => clickButton(container, label);
    try {
      await act(async () =>
        root.render(
          createElement(RunHistoryView, {
            runId: detail.runId,
            source,
            workers: [],
          })
        )
      );
      await flushEffects();
      const details = () => container.querySelector('[aria-label="Failure details"]');
      assert.ok(details(), 'failed runs open their context in the inspector');
      assert.match(details()!.textContent!, /Recorded failures/);
      assert.match(details()!.textContent!, /HTTP 404 — base branch not found/);
      assert.doesNotMatch(details()!.textContent!, /Log not loaded/);
      assert.match(details()!.textContent!, /Additional failure details may be missing/);
      assert.doesNotMatch(details()!.textContent!, /Root cause/);
      if (!durable) assert.match(details()!.textContent!, /Runtime output persistence failed/);
      await click('Go to start');
      assert.equal(details(), null, 'earlier replay must not expose terminal context');
      await click('Go to latest');
      const log = [...container.querySelectorAll('button')].find((button) =>
        button.textContent?.includes('View log · v2:3')
      );
      assert.ok(log);
      await act(async () => log.click());
      assert.equal(details(), null);
      assert.equal(
        container.querySelector<HTMLInputElement>('[aria-label="History position"]')?.value,
        '3'
      );
      assert.match(renderedText(container), /Recorded delivery detail/);
      assert.match(
        container.querySelector('[aria-label="Run inspector"]')!.textContent!,
        /Deliver/
      );
    } finally {
      await act(async () => root.unmount());
      container.remove();
    }
  });

const incompleteHistoryError = () =>
  new ApiError(503, 'history_incomplete', 'The remaining archive could not be retained.');

function unavailableDetail(runtime = true): RunDetail {
  const { detail } = failureHistoryFixture(!runtime);
  return {
    ...detail,
    cursor: 'v2:6',
    history: { ...detail.history, cursor: 'v2:6', complete: false },
    observation: { state: 'incomplete', code: 'history_incomplete' },
  };
}

async function withUnavailableViewer(
  t: TestContext,
  source: RunHistoryReader,
  check: (view: {
    container: HTMLElement;
    browser: ReturnType<typeof installBrowser>;
    render: (next: RunHistoryReader) => Promise<void>;
  }) => Promise<void>,
  savedCursor?: string
) {
  stubWorker(t);
  const browser = installBrowser(t);
  const viewKey = 'unavailable-failure-view';
  if (savedCursor)
    browser.sessionStorage.setItem(
      viewKey,
      JSON.stringify({
        selected: 'run',
        inspector: true,
        execution: '',
        cursor: savedCursor,
        following: false,
      })
    );
  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  const render = async (next: RunHistoryReader) => {
    await act(async () =>
      root.render(
        createElement(RunHistoryView, {
          runId: 'failed-run',
          source: next,
          workers: [],
          viewKey,
        })
      )
    );
    await flushEffects();
  };
  try {
    await render(source);
    await check({ container, browser, render });
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
}

for (const runtime of [true, false])
  test(`current failure remains visible when no history page is available (${runtime ? 'runtime' : 'terminal'})`, async (t) => {
    const detail = unavailableDetail(runtime);
    const source: RunHistoryReader = {
      detail: async () => detail,
      page: async () => {
        throw incompleteHistoryError();
      },
    };
    await withUnavailableViewer(t, source, async ({ container }) => {
      const current = container.querySelector('[aria-label="Current run status"]');
      assert.ok(current);
      assert.match(current.textContent!, /separate from the available recorded playback/);
      assert.match(current.textContent!, /HTTP 404 — base branch not found/);
      assert.match(current.textContent!, /Log not loaded · v2:3/);
      assert.equal(
        [...current.querySelectorAll('button')].find((button) =>
          button.textContent?.includes('View log')
        ),
        undefined,
        'a cursor outside loaded history cannot offer a no-op navigation link'
      );
      assert.match(renderedText(container), /This archive is incomplete/);
      assert.match(renderedText(container), /The remaining archive could not be retained/);
      assert.equal(container.querySelector('.run-status')?.textContent, 'Replay');
      assert.equal(
        container.querySelector('.timeline-caption strong')?.textContent,
        'Before the run'
      );
      assert.equal(
        container.querySelector<HTMLInputElement>('[aria-label="History position"]')?.max,
        '0'
      );
    });
  });

function retainedPrefixSource(): RunHistoryReader {
  const reference = { runId: 'failed-run', node: 'deliver', execution: '1', nodeInstance: '1' };
  return {
    detail: async () => unavailableDetail(),
    page: async (_id, after) => {
      if (after !== 'v2:0') throw incompleteHistoryError();
      return {
        events: [
          { cursor: 'v2:1', event: { kind: 'run_started' } },
          { cursor: 'v2:2', event: { kind: 'node_started', reference, input: null, attempt: 1 } },
        ],
        nextCursor: 'v2:2',
        headCursor: 'v2:6',
        complete: false,
      };
    },
  };
}

test('an unavailable suffix keeps current status separate from replay of the retained prefix', async (t) => {
  await withUnavailableViewer(t, retainedPrefixSource(), async ({ container }) => {
    const current = () => container.querySelector('[aria-label="Current run status"]');
    assert.ok(current());
    assert.equal(
      container.querySelector('.timeline-caption strong')?.textContent,
      'Deliver started'
    );
    assert.equal(container.querySelector('.run-status')?.textContent, 'Replay');
    await clickButton(container, 'Go to start');
    assert.equal(current(), null);
    await clickButton(container, 'Next event');
    assert.equal(current(), null);
    await clickButton(container, 'Next event');
    assert.equal(current(), null, 'the prefix end is still a recorded replay position');
    await clickButton(container, 'Go to latest');
    await flushEffects();
    assert.ok(current(), 'explicit latest intent may show the separately labelled current status');
    assert.match(renderedText(container), /The remaining archive could not be retained/);
  });
});

test('a saved earlier cursor at the retained prefix end does not reveal later failure context', async (t) => {
  await withUnavailableViewer(
    t,
    retainedPrefixSource(),
    async ({ container, browser }) => {
      assert.equal(container.querySelector('[aria-label="Current run status"]'), null);
      assert.equal(
        JSON.parse(browser.sessionStorage.getItem('unavailable-failure-view')!).cursor,
        'v2:2'
      );
      assert.equal(
        container.querySelector<HTMLInputElement>('[aria-label="History position"]')?.value,
        '2'
      );
      await clickButton(container, 'Go to latest');
      await flushEffects();
      assert.ok(container.querySelector('[aria-label="Current run status"]'));
    },
    'v2:2'
  );
});

test('paging in flight does not expose unobserved current failure context', async (t) => {
  let rejectPage: ((reason: unknown) => void) | undefined;
  const source: RunHistoryReader = {
    detail: async () => unavailableDetail(),
    page: () =>
      new Promise((_resolve, reject) => {
        rejectPage = reject;
      }),
  };
  await withUnavailableViewer(t, source, async ({ container, render }) => {
    assert.ok(rejectPage);
    assert.equal(container.querySelector('[aria-label="Current run status"]'), null);
    assert.match(renderedText(container), /Loading history/);
    await act(async () => rejectPage!(incompleteHistoryError()));
    await flushEffects();
    assert.ok(container.querySelector('[aria-label="Current run status"]'));
    await render({
      detail: async () => runDetailFixture('failed-run'),
      page: async () => completePage(),
    });
    assert.equal(container.querySelector('[aria-label="Current run status"]'), null);
    assert.doesNotMatch(renderedText(container), /The remaining archive could not be retained/);
  });
});

test('a bounded history batch does not expose future failure context before an unavailable read', async (t) => {
  let pageReads = 0;
  const source: RunHistoryReader = {
    detail: async () => ({
      ...unavailableDetail(),
      cursor: 'v2:6000',
      history: { ...unavailableDetail().history, cursor: 'v2:6000' },
    }),
    async page(_id, after) {
      pageReads++;
      const offset = Number(after.slice(3));
      if (offset >= 5000) throw incompleteHistoryError();
      return {
        events: Array.from({ length: 1000 }, (_, index) => ({
          cursor: `v2:${offset + index + 1}`,
          event: { kind: 'safe_log', execution: null, stream: 'system', line: 'Retained output' },
        })),
        nextCursor: `v2:${offset + 1000}`,
        headCursor: 'v2:6000',
        complete: false,
      };
    },
  };
  await withUnavailableViewer(t, source, async ({ container }) => {
    assert.equal(pageReads, 5);
    assert.equal(container.querySelector('[aria-label="Current run status"]'), null);
    assert.doesNotMatch(renderedText(container), /The remaining archive could not be retained/);
    await loadMoreHistory(container);
    await flushEffects();
    assert.equal(pageReads, 6);
    assert.ok(container.querySelector('[aria-label="Current run status"]'));
  });
});
