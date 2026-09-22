import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement } from 'react';
import type { Root } from 'react-dom/client';
import { renderToStaticMarkup } from 'react-dom/server';
import { JSDOM } from 'jsdom';
import type { HistoryPage } from './run-history';
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
