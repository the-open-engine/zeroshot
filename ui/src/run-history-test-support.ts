import type { TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { act } from 'react';
import { JSDOM } from 'jsdom';
import type { HistoryPage } from './run-history';
import { createViteTestServer } from './test-support';

export function stubWorker(t: TestContext) {
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

export async function viewerModule(t: TestContext) {
  const server = await createViteTestServer(t);
  const viewer = await server.ssrLoadModule('/src/RunHistoryView.tsx');
  return { RunHistoryView: viewer.RunHistoryView };
}

export const completePage = (): HistoryPage => ({
  events: [],
  nextCursor: 'v2:0',
  headCursor: 'v2:0',
  complete: true,
});

export const renderedText = (container: HTMLElement) => container.textContent ?? '';
export async function clickButton(container: HTMLElement, label: string) {
  const button = container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`);
  assert.ok(button, label);
  await act(async () => button.click());
}
/** Runs open in Steps; the inspector belongs to the graph. */
export async function showGraph(container: HTMLElement) {
  const button = [...container.querySelectorAll<HTMLButtonElement>('.view-toggle button')].find(
    (candidate) => candidate.textContent === 'Graph'
  );
  assert.ok(button, 'Graph');
  await act(async () => button.click());
}
export async function loadMoreHistory(container: HTMLElement) {
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

export function installBrowser(t: TestContext) {
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

export async function flushEffects() {
  await act(async () => {
    await new Promise<void>((resolve) => setImmediate(resolve));
  });
}
