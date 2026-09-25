import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement } from 'react';
import { JSDOM } from 'jsdom';
import type { Bootstrap } from './api';
import type { Document } from './domain';
import type { SaveProfile, WorkspaceServices } from './workspace-services';
import { createViteTestServer } from './test-support';

function installBrowser(t: TestContext) {
  const dom = new JSDOM('<!doctype html><html><body></body></html>', {
    pretendToBeVisual: true,
    url: 'https://example.test/ui/',
  });
  const globals = {
    window: dom.window,
    document: dom.window.document,
    navigator: dom.window.navigator,
    localStorage: dom.window.localStorage,
    sessionStorage: dom.window.sessionStorage,
    Element: dom.window.Element,
    HTMLElement: dom.window.HTMLElement,
    SVGElement: dom.window.SVGElement,
    Node: dom.window.Node,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    ResizeObserver: class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
    Worker: class {
      postMessage() {}
      terminate() {}
    },
  };
  const descriptors = Object.fromEntries(
    Object.keys(globals).map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)])
  );
  for (const [name, value] of Object.entries(globals))
    Object.defineProperty(globalThis, name, { configurable: true, value });
  Object.defineProperty(dom.window, 'ResizeObserver', {
    configurable: true,
    value: globals.ResizeObserver,
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
  dom.window.HTMLDialogElement.prototype.showModal = function () {
    this.open = true;
  };
  dom.window.HTMLDialogElement.prototype.close = function () {
    this.open = false;
  };
  const actGlobal = globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean };
  const originalActEnvironment = actGlobal.IS_REACT_ACT_ENVIRONMENT;
  actGlobal.IS_REACT_ACT_ENVIRONMENT = true;
  t.after(() => {
    actGlobal.IS_REACT_ACT_ENVIRONMENT = originalActEnvironment;
    for (const name of Object.keys(globals)) {
      if (descriptors[name]) Object.defineProperty(globalThis, name, descriptors[name]);
      else Reflect.deleteProperty(globalThis, name);
    }
    dom.window.close();
  });
  return dom.window;
}

const profile = (): Document => ({
  name: 'review-profile',
  graph: {
    profile: 'openengine.graph.full/v1',
    initialInput: { kind: 'null' },
    policy: {},
    root: {
      kind: 'seq',
      name: 'run',
      state: { kind: 'record', fields: {} },
      children: [],
    },
  },
  runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
});

test('mounted App applies graph JSON and saves the edited profile', async (t) => {
  let unmount: (() => Promise<void>) | undefined;
  t.after(async () => unmount?.());
  const browser = installBrowser(t);
  const server = await createViteTestServer(t);
  const { App } = await server.ssrLoadModule('/src/App.tsx');
  const { createRoot } = await import('react-dom/client');
  const bootstrap: Bootstrap = {
    version: 1,
    workspace: { kind: 'local', id: 'app-test' },
    templates: [],
    runtimeSchema: {},
  };
  let resolveSaved!: (profile: SaveProfile) => void;
  const savedCall = new Promise<SaveProfile>((resolve) => {
    resolveSaved = resolve;
  });
  const services: WorkspaceServices = {
    mount: new URL('https://example.test/ui/'),
    catalog: async () => bootstrap,
    profiles: {
      list: async () => ({
        profiles: [{ id: 'profile-1', name: profile().name, isDefault: false }],
      }),
      load: async () => ({
        profile: { ...profile(), id: 'profile-1', isDefault: false },
        revision: 'r1',
      }),
      save: async (next) => {
        resolveSaved(next);
        return { profile: { ...next, id: 'profile-1', isDefault: false }, revision: 'r2' };
      },
    },
    authoring: {
      validate: async () => {},
      outcome: async (document) => document,
      data: async (document) => document,
    },
    history: {} as WorkspaceServices['history'],
  };
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  unmount = async () => {
    await act(async () => root.unmount());
    container.remove();
  };
  await act(async () => root.render(createElement(App, { services, bootstrap })));
  await act(async () => new Promise<void>((resolve) => setImmediate(resolve)));

  const graphButton = container.querySelector<HTMLButtonElement>('[aria-label="Graph JSON"]');
  assert.ok(graphButton);
  await act(async () => graphButton.click());
  const editor = container.querySelector<HTMLTextAreaElement>('[aria-label="JSON source"]');
  assert.ok(editor);
  const graph = JSON.parse(editor.value);
  graph.policy = { reviewed: true };
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(
      browser.HTMLTextAreaElement.prototype,
      'value'
    )?.set;
    assert.ok(setter);
    setter.call(editor, JSON.stringify(graph));
    editor.dispatchEvent(new browser.Event('input', { bubbles: true }));
  });
  const apply = [...container.querySelectorAll<HTMLButtonElement>('button')].find(
    (button) => button.textContent?.trim() === 'Apply changes'
  );
  assert.ok(apply);
  await act(async () => apply.click());
  assert.equal(container.querySelector('[aria-label="JSON source"]'), null);
  assert.match(container.textContent ?? '', /Unsaved/);
  const save = [...container.querySelectorAll<HTMLButtonElement>('button')].find(
    (button) => button.textContent?.trim() === 'Save profile'
  );
  assert.ok(save);
  assert.equal(save.disabled, false);
  let deadline: ReturnType<typeof setTimeout> | undefined;
  try {
    await act(async () => {
      save.click();
      await Promise.race([
        savedCall,
        new Promise<never>((_, reject) => {
          deadline = setTimeout(() => reject(new Error('Profile save was not called.')), 2000);
        }),
      ]);
    });
  } finally {
    if (deadline) clearTimeout(deadline);
  }
  const saved = await savedCall;
  assert.deepEqual(saved.graph.policy, { reviewed: true });
  assert.equal(saved.expectedRevision, 'r1');
  assert.match(container.textContent ?? '', /Profile saved/);
  await unmount();
  unmount = undefined;
});
