import test from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { JSDOM } from 'jsdom';
import {
  preferredLayoutKey,
  readRunView,
  useRunView,
  writeRunView,
  type RunViewState,
} from './run-view-state';
import { workspaceStorageKeys } from './workspace-storage';
import { installTestGlobals } from './test-support';

test('run view preferences isolate workspace, source and run, and ignore damaged or unavailable storage', (t) => {
  const dom = new JSDOM('', { url: 'https://example.test/ui/' });
  t.after(() => dom.window.close());
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', { configurable: true, value: dom.window });
  t.after(() => {
    if (descriptor) Object.defineProperty(globalThis, 'window', descriptor);
    else Reflect.deleteProperty(globalThis, 'window');
  });
  const mount = new URL('https://example.test/ui/');
  const keys = workspaceStorageKeys(mount, { kind: 'target', id: 'target-a' });
  const key = keys.runView('history', 'run-a');
  const view: RunViewState = {
    selected: 'worker',
    inspector: true,
    execution: 'execution:4',
    cursor: 'v2:15',
    following: false,
    canvas: { collapsed: ['reviews'], viewport: { x: 120, y: -10, zoom: 0.8 } },
  };
  writeRunView(key, view);
  assert.deepEqual(readRunView(key), view);
  assert.equal(readRunView(keys.runView('example', 'run-a')), undefined);
  assert.equal(readRunView(keys.runView('history', 'run-b')), undefined);
  assert.equal(
    readRunView(
      workspaceStorageKeys(mount, { kind: 'target', id: 'target-b' }).runView('history', 'run-a')
    ),
    undefined
  );
  for (const damaged of [
    '{',
    'null',
    JSON.stringify({ ...view, cursor: 'invalid' }),
    JSON.stringify({ ...view, canvas: { collapsed: null } }),
  ]) {
    dom.window.sessionStorage.setItem(key, damaged);
    assert.equal(readRunView(key), undefined);
  }
  Object.defineProperty(dom.window, 'sessionStorage', {
    get() {
      throw new Error('blocked');
    },
  });
  assert.equal(readRunView(key), undefined);
  assert.doesNotThrow(() => writeRunView(key, view));
});

test('the preferred run view is shared by runs of one workspace, never across workspaces', () => {
  const mount = new URL('https://example.test/ui/');
  const first = workspaceStorageKeys(mount, { kind: 'target', id: 'target-a' });
  const second = workspaceStorageKeys(mount, { kind: 'target', id: 'target-b' });
  const preference = preferredLayoutKey(first.runView('history', 'run-a'));
  assert.ok(preference);
  assert.equal(preferredLayoutKey(first.runView('example', 'run-b')), preference);
  assert.notEqual(preferredLayoutKey(second.runView('history', 'run-a')), preference);
  assert.equal(preferredLayoutKey(undefined), undefined);
});

for (const { name, mount, ids } of [
  {
    name: 'workspace IDs containing .run-view.',
    mount: 'https://example.test/ui/',
    ids: ['team.run-view.alpha', 'team.run-view.beta'],
  },
  {
    name: 'a hostname containing .run-view.',
    mount: 'https://ui.run-view.example.test/ui/',
    ids: ['workspace-a', 'workspace-b'],
  },
  {
    name: 'an encoded array boundary inside the workspace ID',
    mount: 'https://example.test/ui/',
    ids: ['team].run-view.[alpha', 'team].run-view.[beta'],
  },
]) {
  test(`mounted layout preferences isolate ${name}`, async () => {
    const dom = new JSDOM('<div id="root"></div>', { url: mount });
    const restore = installTestGlobals({
      window: dom.window,
      document: dom.window.document,
      IS_REACT_ACT_ENVIRONMENT: true,
    });
    let view: ReturnType<typeof useRunView>;
    function Viewer({ viewKey }: { viewKey: string }) {
      view = useRunView(viewKey);
      return createElement('span', null, view.layout);
    }
    const workspaces = ids.map((id) =>
      workspaceStorageKeys(new URL(mount), { kind: 'platform', id })
    );
    const container = dom.window.document.getElementById('root')!;
    let root = createRoot(container);
    const open = async (workspace: number, run: string) => {
      await act(async () => root.unmount());
      root = createRoot(container);
      await act(async () =>
        root.render(
          createElement(Viewer, { viewKey: workspaces[workspace].runView('history', run) })
        )
      );
    };
    try {
      await open(0, 'first-run');
      assert.equal(container.textContent, 'story');
      await act(async () => view.setLayout('graph'));
      assert.equal(container.textContent, 'graph');
      await open(1, 'first-run');
      assert.equal(container.textContent, 'story', 'another workspace still defaults to Steps');
      await act(async () => view.setLayout('story'));
      await open(0, 'another].run-view.[run');
      assert.equal(container.textContent, 'graph', 'new runs remember their own workspace layout');
    } finally {
      await act(async () => root.unmount());
      restore();
      dom.window.close();
    }
  });
}
