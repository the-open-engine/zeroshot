import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement } from 'react';
import { JSDOM } from 'jsdom';
import { createViteTestServer } from './test-support';
import type { EnvironmentStore, Environment } from './environment-store';
import type { HostCommand } from './workspace-bridge';
import { workspaceStorageKeys } from './workspace-storage';

async function renderWorkspace(t: TestContext, store: EnvironmentStore, readOnly = false) {
  const dom = new JSDOM('<!doctype html><div id="root"></div>', {
    url: 'https://example.test/ui/',
    pretendToBeVisual: true,
  });
  const globals = {
    window: dom.window,
    document: dom.window.document,
    sessionStorage: dom.window.sessionStorage,
    HTMLElement: dom.window.HTMLElement,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    IS_REACT_ACT_ENVIRONMENT: true,
  };
  const previous = new Map(
    Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)])
  );
  for (const [key, value] of Object.entries(globals))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  dom.window.confirm = () => true;
  const server = await createViteTestServer(t);
  const { EnvironmentWorkspace } = await server.ssrLoadModule('/src/EnvironmentWorkspace.tsx');
  const { createRoot } = await import('react-dom/client');
  const container = dom.window.document.getElementById('root')!;
  let root = createRoot(container);
  const { WorkspaceBridge } = await server.ssrLoadModule('/src/workspace-bridge.ts');
  const messages: any[] = [];
  let bridge = initializeBridge(WorkspaceBridge, messages);
  async function mount(workspaceId = 'test') {
    await act(async () =>
      root.render(
        createElement(EnvironmentWorkspace, {
          services: { mount: new URL('https://example.test/ui/'), environments: store },
          bootstrap: { workspace: { kind: 'local', id: workspaceId } },
          host: bridge.host,
          readOnly,
        })
      )
    );
  }
  await mount();
  t.after(async () => {
    await act(async () => root.unmount());
    bridge.host.dispose();
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  });
  const label = (name: string) => fieldLabel(dom, name);
  const button = (name: string) => namedButton(dom, name);
  return {
    dom,
    messages,
    remount: async (workspaceId = 'test') => {
      await act(async () => root.unmount());
      bridge.host.dispose();
      bridge = initializeBridge(WorkspaceBridge, messages, workspaceId);
      root = createRoot(container);
      await mount(workspaceId);
    },
    command: async (command: Partial<HostCommand>) =>
      act(async () =>
        bridge.receive({
          ...messages.at(-1),
          ...command,
          requestId: 'nav',
          type: 'navigate',
          action: 'leave',
        })
      ),
    label,
    button,
    click: async (name: string) => act(async () => button(name).click()),
    input: async (name: string, value: string, type = 'input') =>
      act(async () => {
        const field = label(name);
        const prototype =
          field.tagName === 'SELECT'
            ? dom.window.HTMLSelectElement.prototype
            : field.tagName === 'TEXTAREA'
              ? dom.window.HTMLTextAreaElement.prototype
              : dom.window.HTMLInputElement.prototype;
        Object.getOwnPropertyDescriptor(prototype, 'value')!.set!.call(field, value);
        field.dispatchEvent(new dom.window.Event(type, { bubbles: true }));
      }),
    submit: async () =>
      act(async () => {
        dom.window.document
          .querySelector('form')!
          .dispatchEvent(new dom.window.Event('submit', { bubbles: true, cancelable: true }));
        await new Promise((resolve) => setTimeout(resolve, 40));
      }),
  };
}
function fixture() {
  let saved: Environment = {
    id: 'env-1',
    name: 'node-tools',
    definition: { startup: 'npm ci' },
    revision: 'r1',
  };
  const writes: any[] = [];
  const store: EnvironmentStore = {
    list: async () => ({ environments: [saved] }),
    load: async () => structuredClone(saved),
    save: async (value) => {
      writes.push(structuredClone(value));
      saved = {
        id: value.id ?? 'env-2',
        name: value.name,
        definition: value.definition,
        revision: 'r2',
      };
      return structuredClone(saved);
    },
    remove: async () => {},
  };
  return { store, writes };
}

test('environment edits save their own CAS resource and protect unsaved navigation', async (t) => {
  const { ui, writes } = await editingFixture(t);
  await ui.input('Startup script', 'npm ci --ignore-scripts');
  await ui.command({});
  assert.equal(ui.messages.at(-1).problem.code, 'unsaved_changes');
  await ui.submit();
  assert.deepEqual(writes, [
    {
      id: 'env-1',
      name: 'node-tools',
      definition: { startup: 'npm ci --ignore-scripts' },
      expectedRevision: 'r1',
    },
  ]);
  assert.match(ui.dom.window.document.body.textContent!, /Environment saved/);
  assert.equal(ui.messages.at(-1).dirty, false);
  assert.equal(ui.button('Add variable').type, 'button');
});

test('focused pending variable names are committed before capturing the environment save', async (t) => {
  const { ui, writes } = await editingFixture(t);
  await ui.click('Add variable');
  await act(async () => ui.label('Variable VARIABLE name').focus());
  await ui.input('Variable VARIABLE name', 'PROJECT_MODE');
  await ui.submit();
  assert.equal(writes.length, 1);
  assert.deepEqual(writes[0].definition.variables, { PROJECT_MODE: '' });
});

test('failed save and referenced deletion retain the environment draft and saved revision', async (t) => {
  const { store } = fixture();
  store.save = async () => {
    throw new Error('Environment changed. Reload before saving.');
  };
  store.remove = async () => {
    throw new Error('This environment is used by a profile.');
  };
  const ui = await renderWorkspace(t, store);
  await ui.input('Select environment', 'env-1', 'change');
  await ui.input('Startup script', 'local unsaved work');
  await ui.submit();
  assert.equal(ui.label('Startup script').value, 'local unsaved work');
  assert.match(ui.dom.window.document.body.textContent!, /Environment changed/);
  ui.dom.window.confirm = () => false;
  await ui.click('Reload environment');
  assert.equal(ui.label('Startup script').value, 'local unsaved work');
  ui.dom.window.confirm = () => true;
  await ui.click('Delete environment');
  assert.match(ui.dom.window.document.body.textContent!, /used by a profile/);
  assert.equal(ui.label('Startup script').value, 'local unsaved work');
  await ui.click('Reload environment');
  assert.equal(ui.label('Startup script').value, 'npm ci');
});

test('read-only environment view can select resources but cannot modify or delete them', async (t) => {
  const { store, writes } = fixture();
  const ui = await renderWorkspace(t, store, true);
  await ui.input('Select environment', 'env-1', 'change');
  assert.equal(ui.dom.window.document.querySelector('fieldset')!.disabled, true);
  const text = ui.dom.window.document.body.textContent!;
  assert.doesNotMatch(text, /New environment|Save environment|Delete environment/);
  assert.equal(ui.label('Startup script').value, 'npm ci');
  assert.deepEqual(writes, []);
});

function fieldLabel(dom: JSDOM, name: string) {
  const explicit = dom.window.document.querySelector(`[aria-label="${name}"]`);
  if (explicit) return explicit as HTMLInputElement;
  const field = [...dom.window.document.querySelectorAll('label')].find(
    (el) => el.textContent?.trim() === name
  );
  assert.ok(field, `Missing field ${name}`);
  return dom.window.document.getElementById(field.htmlFor) as HTMLInputElement;
}
function namedButton(dom: JSDOM, name: string) {
  const value = [...dom.window.document.querySelectorAll('button')].find(
    (el) => el.textContent?.trim() === name
  );
  assert.ok(value, `Missing button ${name}`);
  return value;
}

function initializeBridge(Bridge: any, messages: any[], workspaceId = 'test') {
  const origin = 'https://example.test';
  const parent = { postMessage: (value: unknown) => messages.push(structuredClone(value)) };
  let incoming: (event: unknown) => void = () => {};
  const host = new Bridge(
    origin,
    parent,
    (listener: typeof incoming) => {
      incoming = listener;
      return () => {};
    },
    () => {}
  );
  const receive = (data: unknown) => incoming({ origin, source: parent, data });
  receive({
    version: 1,
    type: 'init',
    requestId: host.readyId,
    workspaceId,
    authority: { userId: 'user', organizationId: 'org', scope: 'user' },
    apiBase: '/ui/api/',
    theme: 'light',
    view: 'environments',
  });
  assert.ok(host.configuration, 'The real workspace bridge must accept host initialization');
  return { host, receive };
}

test('remount restores committed edits and their original CAS revision', async (t) => {
  const { ui, writes } = await editingFixture(t);
  await ui.input('Startup script', 'preserve this edited startup');
  await ui.remount();
  assert.equal(ui.label('Startup script').value, 'preserve this edited startup');
  await ui.submit();
  assert.equal(writes[0].expectedRevision, 'r1');
  assert.equal(writes[0].definition.startup, 'preserve this edited startup');
  await ui.remount();
  assert.equal(ui.dom.window.document.querySelector('textarea'), null, 'A saved draft is cleared');
});

test('recovery is scoped to its workspace and explicit discard clears the current draft', async (t) => {
  const { store } = fixture();
  const ui = await renderWorkspace(t, store);
  await ui.input('Select environment', 'env-1', 'change');
  await ui.input('Startup script', 'original workspace draft');
  await ui.remount('another-scope');
  assert.equal(ui.dom.window.document.querySelector('textarea'), null);
  await ui.remount();
  assert.equal(ui.label('Startup script').value, 'original workspace draft');
  await ui.command({ discard: true });
  assert.equal(ui.messages.at(-1).dirty, false);
  await ui.remount();
  assert.equal(
    ui.dom.window.document.querySelector('textarea'),
    null,
    'Explicit discard clears recovery'
  );
});

test('deletion clears recovery and malformed storage cannot prevent opening environments', async (t) => {
  const { store } = fixture();
  const ui = await renderWorkspace(t, store);
  await ui.input('Select environment', 'env-1', 'change');
  await ui.input('Startup script', 'edited before deletion');
  await ui.click('Delete environment');
  await ui.remount();
  assert.equal(ui.dom.window.document.querySelector('textarea'), null);
  const key = workspaceStorageKeys(new URL('https://example.test/ui/'), {
    kind: 'local',
    id: 'test',
  }).environmentDraft;
  ui.dom.window.sessionStorage.setItem(key, '{corrupt');
  await ui.remount();
  assert.equal(ui.dom.window.document.querySelector('textarea'), null);
  assert.equal(ui.button('New environment').disabled, false);
});

async function editingFixture(t: TestContext) {
  const { store, writes } = fixture();
  const ui = await renderWorkspace(t, store);
  await ui.input('Select environment', 'env-1', 'change');
  return { ui, writes };
}

test('large escaped environment definitions recover with their CAS base within the shared bound', async (t) => {
  const { store, writes } = fixture();
  const definition = {
    setup: '\u0001'.repeat(64 * 1024),
    startup: '\u0001'.repeat(64 * 1024),
    variables: { FIRST: '\u0001'.repeat(128 * 1024 - 5), SECOND: '\u0001'.repeat(128 * 1024 - 6) },
  };
  assert.equal(Buffer.byteLength(definition.setup), 64 * 1024);
  assert.equal(Buffer.byteLength(definition.startup), 64 * 1024);
  assert.equal(
    Object.entries(definition.variables).reduce(
      (size, [name, value]) => size + Buffer.byteLength(name + value),
      0
    ),
    256 * 1024
  );
  assert.ok(Buffer.byteLength(JSON.stringify(definition)) > 2 * 1024 * 1024);
  store.load = async () => ({ id: 'env-1', name: 'large-env', definition, revision: 'large-r1' });
  const ui = await renderWorkspace(t, store);
  await ui.input('Select environment', 'env-1', 'change');
  const edited = definition.startup.slice(0, -1) + '\u0002';
  await ui.input('Startup script', edited);
  const key = workspaceStorageKeys(new URL('https://example.test/ui/'), {
    kind: 'local',
    id: 'test',
  }).environmentDraft;
  const snapshot = ui.dom.window.sessionStorage.getItem(key)!;
  assert.ok(
    Buffer.byteLength(snapshot) > 4 * 1024 * 1024,
    'The regression must exceed the former read limit'
  );
  await ui.remount();
  assert.equal(ui.label('Startup script').value, edited);
  await ui.submit();
  assert.equal(writes[0].expectedRevision, 'large-r1');
  assert.equal(ui.dom.window.sessionStorage.getItem(key), null);
});

test('oversized unsaved drafts report recovery unavailable and remove stale recovery', async (t) => {
  const { ui } = await editingFixture(t);
  await ui.input('Startup script', 'small recoverable draft');
  const key = workspaceStorageKeys(new URL('https://example.test/ui/'), {
    kind: 'local',
    id: 'test',
  }).environmentDraft;
  assert.ok(ui.dom.window.sessionStorage.getItem(key));
  await ui.input('Startup script', 'x'.repeat(8 * 1024 * 1024));
  assert.match(ui.dom.window.document.body.textContent!, /Draft recovery is unavailable/);
  assert.equal(ui.dom.window.sessionStorage.getItem(key), null);
});
