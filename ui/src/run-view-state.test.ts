import test from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { readRunView, writeRunView, type RunViewState } from './run-view-state';
import { workspaceStorageKeys } from './workspace-storage';

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
