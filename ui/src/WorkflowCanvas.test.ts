import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { act, createElement } from 'react';
import {
  ReactFlowProvider,
  getViewportForBounds,
  type Viewport,
  type XYPosition,
} from '@xyflow/react';
import { JSDOM } from 'jsdom';
import type { GraphNode } from './domain';
import type { CanvasView } from './run-view-state';
import { createViteTestServer, installTestGlobals, runDetailFixture } from './test-support';

type ElkMessage = { data: unknown };
type FakeWorker = {
  onmessage?: (message: ElkMessage) => void;
  postMessage(message: unknown): void;
};
// ELK's in-process stand-in for its browser worker, so layouts finish inside the test.
const { Worker: ElkWorker } = createRequire(import.meta.url)('elkjs/lib/elk-worker.min.js') as {
  Worker: new () => FakeWorker;
};

const root: GraphNode = {
  kind: 'seq',
  name: 'run',
  children: [
    { kind: 'step', name: 'prepare' },
    {
      kind: 'loop',
      name: 'rounds',
      maxIterations: 2,
      body: {
        kind: 'seq',
        name: 'round',
        children: [
          { kind: 'step', name: 'write' },
          { kind: 'step', name: 'review' },
        ],
      },
    },
    { kind: 'step', name: 'verify' },
    { kind: 'step', name: 'ship' },
    { kind: 'step', name: 'announce' },
  ],
};
const saved = { collapsed: ['rounds'], viewport: { x: 40, y: 30, zoom: 1.25 } };
const canvas = { width: 800, height: 600 };

/** Runs layouts through ELK; `hold` keeps new ones waiting until `release`. */
function layoutWorker() {
  const waiting: (() => void)[] = [];
  let holding = false;
  return {
    Worker: class {
      onmessage?: (message: ElkMessage) => void;
      #elk = new ElkWorker();
      constructor() {
        this.#elk.onmessage = (message) => this.onmessage?.(message);
      }
      postMessage(message: unknown) {
        if (holding) waiting.push(() => this.#elk.postMessage(message));
        else this.#elk.postMessage(message);
      }
    },
    waiting: () => waiting.length,
    hold() {
      holding = true;
    },
    release() {
      holding = false;
      waiting.splice(0).forEach((send) => send());
    },
  };
}

type Measured = { target: HTMLElement; contentRect: { width: number; height: number } };
/** Reports an element when first observed and whenever its inline size changes, as a browser does. */
class MeasuringObserver {
  #sizes = new Map<HTMLElement, string>();
  #styles?: MutationObserver;
  #report: (entries: Measured[]) => void;
  constructor(report: (entries: Measured[]) => void) {
    this.#report = report;
  }
  observe(target: HTMLElement) {
    this.#styles ??= new target.ownerDocument.defaultView!.MutationObserver(() => this.#measure());
    this.#styles.observe(target, { attributeFilter: ['style'] });
    this.#sizes.set(target, '');
    queueMicrotask(() => this.#measure());
  }
  unobserve(target: HTMLElement) {
    this.#sizes.delete(target);
  }
  disconnect() {
    this.#sizes.clear();
    this.#styles?.disconnect();
  }
  #measure() {
    const entries: Measured[] = [];
    for (const [target, last] of this.#sizes) {
      const contentRect = { width: target.offsetWidth, height: target.offsetHeight };
      const size = `${contentRect.width}x${contentRect.height}`;
      if (size === last) continue;
      this.#sizes.set(target, size);
      entries.push({ target, contentRect });
    }
    if (entries.length) this.#report(entries);
  }
}

/** Keep the prior measurements while a newly drawn layout waits for ResizeObserver. */
function delayedMeasurements() {
  let holding = false;
  const waiting: (() => void)[] = [];
  return {
    Observer: class extends MeasuringObserver {
      constructor(report: (entries: Measured[]) => void) {
        super((entries) => {
          if (holding) waiting.push(() => report(entries));
          else report(entries);
        });
      }
    },
    waiting: () => waiting.length,
    hold() {
      holding = true;
    },
    release() {
      holding = false;
      waiting.splice(0).forEach((report) => report());
    },
  };
}

/** The zoom React Flow reads back from the viewport transform when it measures a node. */
class ScaleMatrix {
  m22: number;
  constructor(transform = '') {
    this.m22 = Number(/scale\((.+?)\)/.exec(transform)?.[1] ?? 1);
  }
}

type CanvasOptions = {
  root?: GraphNode;
  readOnly?: boolean;
  view?: CanvasView;
  size?: typeof canvas;
  observer?: typeof MeasuringObserver;
  toolbarStart?: ReturnType<typeof createElement>;
};

/** A page where React Flow measures its nodes and ELK lays them out. */
function installCanvasBrowser(
  worker: unknown,
  frames?: (callback: FrameRequestCallback) => number,
  options: CanvasOptions = {}
) {
  const size = options.size ?? canvas;
  const dom = new JSDOM(
    '<!doctype html><style>.react-flow__pane { z-index: 1; }</style><div id="root"></div>',
    { pretendToBeVisual: true, url: 'https://example.test/' }
  );
  Object.defineProperty(dom.window, 'DOMMatrixReadOnly', {
    configurable: true,
    value: ScaleMatrix,
  });
  const px = (value: string, fallback: number) =>
    value.endsWith('px') ? parseFloat(value) : fallback;
  Object.defineProperties(dom.window.HTMLElement.prototype, {
    clientWidth: { configurable: true, get: () => size.width },
    clientHeight: { configurable: true, get: () => size.height },
    offsetWidth: {
      configurable: true,
      get(this: HTMLElement) {
        return px(this.style.width, size.width);
      },
    },
    offsetHeight: {
      configurable: true,
      get(this: HTMLElement) {
        return px(this.style.height, size.height);
      },
    },
  });
  const restoreGlobals = installTestGlobals({
    window: dom.window,
    document: dom.window.document,
    navigator: dom.window.navigator,
    Element: dom.window.Element,
    HTMLElement: dom.window.HTMLElement,
    SVGElement: dom.window.SVGElement,
    Node: dom.window.Node,
    requestAnimationFrame: frames ?? dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    getComputedStyle: dom.window.getComputedStyle.bind(dom.window),
    ResizeObserver: options.observer ?? MeasuringObserver,
    Worker: worker,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  return {
    container: dom.window.document.getElementById('root')!,
    restore() {
      restoreGlobals();
      dom.window.close();
    },
  };
}

/** Mounts a canvas from its saved view and waits for the first layout. */
async function mountRunGraph(
  t: TestContext,
  layouts: ReturnType<typeof layoutWorker>,
  frames?: (callback: FrameRequestCallback) => number,
  options: CanvasOptions = {}
) {
  const { container, restore } = installCanvasBrowser(layouts.Worker, frames, options);
  let unmount = async () => {};
  t.after(async () => {
    await unmount();
    restore();
  });
  const server = await createViteTestServer(t);
  const { WorkflowCanvas } = await server.ssrLoadModule('/src/WorkflowCanvas.tsx');
  const { createRoot } = await import('react-dom/client');
  const reactRoot = createRoot(container);
  unmount = () => act(async () => reactRoot.unmount());
  const detail = runDetailFixture();
  const view = options.view ?? saved;
  const document = {
    name: detail.runId,
    graph: { ...detail.graph, root: options.root ?? root },
    runtime: detail.runtime,
  };
  await act(async () =>
    reactRoot.render(
      createElement(
        ReactFlowProvider,
        null,
        createElement(WorkflowCanvas, {
          document,
          selected: '',
          positions: {},
          setPositions: () => {},
          workers: [],
          select: () => {},
          observation: options.readOnly === false ? undefined : { key: detail.runId, nodes: {} },
          view,
          inspectorHidden: false,
          showInspector: () => {},
          toolbarStart: options.toolbarStart,
        })
      )
    )
  );
  await settle(container, () =>
    matchesViewport(container, view.viewport ?? fitted(container, options.size ?? canvas))
  );
  return container;
}

const transform = (container: HTMLElement) =>
  container.querySelector<HTMLElement>('.react-flow__viewport')?.style.transform ?? '';
const numbers = (pattern: RegExp, text: string) =>
  (pattern.exec(text) ?? assert.fail(`no match in "${text}"`)).slice(1).map(Number);

/** Wait for the requested view as well as layout completion and a quiet viewport. */
async function settle(container: HTMLElement, ready: () => boolean = () => true) {
  let last = transform(container);
  let quietSince = Date.now();
  for (const deadline = Date.now() + 5_000; Date.now() < deadline;) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    const current = transform(container);
    if (container.querySelector('.workflow-arranging') || current !== last || !ready()) {
      last = current;
      quietSince = Date.now();
    } else if (Date.now() - quietSince >= 150) return;
  }
  assert.fail('the canvas did not reach a settled view');
}

async function click(container: HTMLElement, label: string) {
  const button = container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`);
  assert.ok(button, label);
  await act(async () => button.click());
}

function viewport(container: HTMLElement): Viewport {
  const [x, y, zoom] = numbers(
    /translate\((.+?)px, ?(.+?)px\) scale\((.+?)\)/,
    transform(container)
  );
  return { x, y, zoom };
}

/** The graph-space box of a drawn node. */
function box(node: HTMLElement | null | undefined) {
  assert.ok(node);
  const [x, y] = numbers(/translate\((.+?)px, ?(.+?)px\)/, node.style.transform);
  return { x, y, width: parseFloat(node.style.width), height: parseFloat(node.style.height) };
}
const nodeWith = (container: HTMLElement, label: string) =>
  container.querySelector(`[aria-label="${label}"]`)?.closest<HTMLElement>('.react-flow__node');
const onScreen = (point: XYPosition, view: Viewport) => ({
  x: Math.round((point.x * view.zoom + view.x) * 10) / 10,
  y: Math.round((point.y * view.zoom + view.y) * 10) / 10,
});

/** The graph-space bounds of everything drawn. */
function drawnBounds(container: HTMLElement) {
  const boxes = [...container.querySelectorAll<HTMLElement>('.react-flow__node')].map(box);
  const x = Math.min(...boxes.map((drawn) => drawn.x));
  const y = Math.min(...boxes.map((drawn) => drawn.y));
  return {
    x,
    y,
    width: Math.max(...boxes.map((drawn) => drawn.x + drawn.width)) - x,
    height: Math.max(...boxes.map((drawn) => drawn.y + drawn.height)) - y,
  };
}
/** The view the canvas fits the whole graph into, with its padding and zoom limits. */
const fitted = (container: HTMLElement, size = canvas) =>
  getViewportForBounds(drawnBounds(container), size.width, size.height, 0.15, 0.9, 0.12);
const rounded = (view: Viewport) => ({
  x: Math.round(view.x * 10) / 10,
  y: Math.round(view.y * 10) / 10,
  zoom: Math.round(view.zoom * 1000) / 1000,
});

const matchesViewport = (container: HTMLElement, expected: Viewport) => {
  const actual = rounded(viewport(container));
  const target = rounded(expected);
  return actual.x === target.x && actual.y === target.y && actual.zoom === target.zoom;
};

/** Expands `rounds` and checks its region appears where its card was, at the same zoom. */
async function expandInPlace(container: HTMLElement) {
  const before = viewport(container);
  const card = onScreen(box(nodeWith(container, 'Expand rounds')), before);
  await click(container, 'Expand rounds');
  await settle(container);
  const after = viewport(container);
  assert.equal(after.zoom, before.zoom);
  assert.deepEqual(onScreen(box(nodeWith(container, 'Collapse rounds')), after), card);
}

test('a host without its own header gets its controls beside the settings button', async (t) => {
  const container = await mountRunGraph(t, layoutWorker(), undefined, {
    readOnly: false,
    toolbarStart: createElement('button', { 'aria-label': 'Undo' }),
  });
  const start = container.querySelector('.canvas-toolbar .canvas-start');
  assert.match(start?.textContent ?? '', /Run settings/);
  assert.ok(start?.querySelector('.canvas-host-actions [aria-label="Undo"]'));
  assert.equal(container.querySelectorAll('.canvas-host-actions').length, 1);
});

test('expanding and collapsing a group keep it where it was on screen, at the same zoom', async (t) => {
  const container = await mountRunGraph(t, layoutWorker());
  assert.deepEqual(viewport(container), saved.viewport);
  await expandInPlace(container);

  const expanded = viewport(container);
  const region = onScreen(box(nodeWith(container, 'Collapse rounds')), expanded);
  await click(container, 'Collapse rounds');
  await settle(container);
  const collapsed = viewport(container);
  assert.equal(collapsed.zoom, saved.viewport.zoom);
  assert.deepEqual(onScreen(box(nodeWith(container, 'Expand rounds')), collapsed), region);
});

test('placing a toggled group does not wait on frames that may beat the drawn layout', async (t) => {
  // Frames that fire before React has drawn the new layout: the worst case of the race in a browser.
  const early = (callback: FrameRequestCallback) => {
    queueMicrotask(() => callback(performance.now()));
    return 0;
  };
  const container = await mountRunGraph(t, layoutWorker(), early);
  await expandInPlace(container);
});

test('collapsing all groups while an expand is still laying out fits the whole graph', async (t) => {
  const layouts = layoutWorker();
  const container = await mountRunGraph(t, layouts);

  layouts.hold();
  await click(container, 'Expand rounds');
  assert.equal(layouts.waiting(), 1, 'the expand is laying out');
  await click(container, 'Collapse all groups');
  layouts.release();
  await settle(container, () => matchesViewport(container, fitted(container)));

  assert.ok(nodeWith(container, 'Expand rounds'), 'every group is collapsed again');
  const view = viewport(container);
  for (const node of container.querySelectorAll<HTMLElement>('.react-flow__node')) {
    const drawn = box(node);
    const topLeft = onScreen(drawn, view);
    const bottomRight = onScreen({ x: drawn.x + drawn.width, y: drawn.y + drawn.height }, view);
    assert.ok(
      topLeft.x >= 0 &&
        topLeft.y >= 0 &&
        bottomRight.x <= canvas.width &&
        bottomRight.y <= canvas.height,
      `${node.dataset.id} is outside the view: ${JSON.stringify({ topLeft, bottomRight, view })}`
    );
  }
});

test('fitting the view while an expand is still laying out fits the finished layout', async (t) => {
  const layouts = layoutWorker();
  const container = await mountRunGraph(t, layouts);

  layouts.hold();
  await click(container, 'Expand rounds');
  assert.equal(layouts.waiting(), 1, 'the expand is laying out');
  await click(container, 'Fit workflow');
  layouts.release();
  await settle(container, () => matchesViewport(container, fitted(container)));

  assert.ok(nodeWith(container, 'Collapse rounds'), 'the group is expanded');
  assert.deepEqual(rounded(viewport(container)), rounded(fitted(container)));
});

for (const branches of [4, 6]) {
  for (const width of [1106, 722]) {
    test(`pending Fit includes ${branches} parallel agents at ${width}px before measurements catch up`, async (t) => {
      const layouts = layoutWorker();
      const measurements = delayedMeasurements();
      const size = { width, height: 644 };
      const profile: GraphNode = {
        kind: 'seq',
        name: 'run',
        children: [
          { kind: 'step', name: 'prepare' },
          {
            kind: 'loop',
            name: 'rounds',
            maxIterations: 2,
            body: {
              kind: 'par',
              name: 'checks',
              join: { kind: 'all' },
              branches: Array.from({ length: branches }, (_, index) => ({
                kind: 'step',
                name: `writer_${index}`,
              })),
            },
          },
          { kind: 'succeed', name: 'done', output: { kind: 'null' }, bindings: [] },
        ],
      };
      const container = await mountRunGraph(t, layouts, undefined, {
        root: profile,
        readOnly: false,
        view: { ...saved, collapsed: [] },
        size,
        observer: measurements.Observer,
      });
      await click(container, 'Collapse rounds');
      await settle(container);

      layouts.hold();
      await click(container, 'Expand rounds');
      assert.equal(layouts.waiting(), 1, 'the expand is laying out');
      // The temporary expanded nodes have been measured, but ELK has not placed them yet.
      await act(async () => new Promise((resolve) => setTimeout(resolve, 40)));
      measurements.hold();
      await click(container, 'Fit workflow');
      layouts.release();
      await settle(container, () => matchesViewport(container, fitted(container, size)));

      assert.ok(measurements.waiting() > 0, 'the completed layout still has stale measurements');
      assert.deepEqual(rounded(viewport(container)), rounded(fitted(container, size)));
      const bounds = drawnBounds(container);
      const view = viewport(container);
      const topLeft = onScreen(bounds, view);
      const bottomRight = onScreen(
        { x: bounds.x + bounds.width, y: bounds.y + bounds.height },
        view
      );
      assert.ok(
        topLeft.x >= 0 &&
          topLeft.y >= 0 &&
          bottomRight.x <= size.width &&
          bottomRight.y <= size.height
      );

      await act(async () => measurements.release());
      await settle(container, () => matchesViewport(container, fitted(container, size)));
      assert.deepEqual(rounded(viewport(container)), rounded(fitted(container, size)));
    });
  }
}

test('saved view and Collapse all complete even when animation frames are delayed', async (t) => {
  const late = (callback: FrameRequestCallback) =>
    setTimeout(() => callback(performance.now()), 250) as unknown as number;
  const layouts = layoutWorker();
  const container = await mountRunGraph(t, layouts, late);
  assert.deepEqual(viewport(container), saved.viewport);

  layouts.hold();
  await click(container, 'Expand rounds');
  await click(container, 'Collapse all groups');
  layouts.release();
  await settle(container, () => matchesViewport(container, fitted(container)));
  assert.deepEqual(rounded(viewport(container)), rounded(fitted(container)));
});
