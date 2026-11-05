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
  const restoreGlobals = installDomGlobals(dom);
  dom.window.confirm = () => true;
  const server = await createViteTestServer(t);
  const { EnvironmentWorkspace } = await server.ssrLoadModule('/src/EnvironmentWorkspace.tsx');
  const { createRoot } = await import('react-dom/client');
  const container = dom.window.document.getElementById('root')!;
  const { WorkspaceBridge } = await server.ssrLoadModule('/src/workspace-bridge.ts');
  const messages: any[] = [];
  const session = {
    root: createRoot(container),
    bridge: initializeBridge(WorkspaceBridge, messages),
  };
  async function mount(workspaceId = 'test', context = '') {
    await act(async () =>
      session.root.render(
        createElement(EnvironmentWorkspace, {
          store,
          collection: new URL(`https://example.test/ui/${context}`),
          bootstrap: { workspace: { kind: 'local', id: workspaceId } },
          host: session.bridge.host,
          readOnly,
        })
      )
    );
  }
  await mount();
  t.after(async () => {
    await act(async () => session.root.unmount());
    session.bridge.host.dispose();
    dom.window.close();
    restoreGlobals();
  });
  return workspaceControls({
    dom,
    messages,
    session,
    WorkspaceBridge,
    createRoot,
    container,
    mount,
  });
}

function installDomGlobals(dom: JSDOM) {
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
  return () => {
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  };
}

function workspaceControls(context: {
  dom: JSDOM;
  messages: any[];
  session: { root: any; bridge: any };
  WorkspaceBridge: any;
  createRoot: (container: Element | DocumentFragment) => any;
  container: HTMLElement;
  mount: (workspaceId?: string, context?: string) => Promise<void>;
}) {
  const { dom, messages, session, WorkspaceBridge, createRoot, container, mount } = context;
  const label = (name: string) => fieldLabel(dom, name);
  const button = (name: string) => namedButton(dom, name);
  const last = (type: string) => messages.filter((message) => message.type === type).at(-1);
  const acknowledge = async (
    snapshot: any,
    result: { environment?: Environment; problem?: { code: string; message: string } }
  ) =>
    act(async () => {
      session.bridge.receive({ ...saveEnvelope(snapshot), type: 'save_ack', ...result });
      await pause(20);
    });
  return {
    dom,
    messages,
    remount: async (workspaceId = 'test', context = '') => {
      await act(async () => session.root.unmount());
      session.bridge.host.dispose();
      session.bridge = initializeBridge(WorkspaceBridge, messages, workspaceId);
      session.root = createRoot(container);
      await mount(workspaceId, context);
    },
    send: async (command: Partial<HostCommand>) =>
      act(async () => {
        session.bridge.receive({
          ...last('state'),
          requestId: crypto.randomUUID(),
          ...command,
        });
        await pause(40);
      }),
    command: async (command: Partial<HostCommand>) =>
      act(async () => {
        session.bridge.receive({
          ...last('state'),
          ...command,
          requestId: 'nav',
          type: 'navigate',
          action: 'leave',
        });
        await pause(20);
      }),
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
    requestSave: async (withAcknowledgement = true) => {
      const requestId = crypto.randomUUID();
      await act(async () => {
        session.bridge.receive({
          ...last('state'),
          requestId,
          type: 'request_save',
        });
        await pause(40);
      });
      const snapshot = messages
        .filter((message) => message.type === 'save_snapshot' && message.requestId === requestId)
        .at(-1);
      assert.ok(snapshot, 'The environment workspace must capture a save snapshot');
      if (withAcknowledgement) {
        const environment = snapshot.environment;
        await acknowledge(snapshot, {
          environment: {
            id: environment.id ?? 'env-2',
            name: environment.name,
            definition: environment.definition,
            revision: 'r2',
          },
        });
      }
      return snapshot;
    },
    acknowledge,
  };
}
const pause = (milliseconds: number) => new Promise((resolve) => setTimeout(resolve, milliseconds));
function saveEnvelope(value: any) {
  const { version, workspaceId, documentId, generation, requestId } = value;
  return { version, workspaceId, documentId, generation, requestId };
}
function fixture() {
  let saved: Environment = {
    id: 'env-1',
    name: 'node-tools',
    definition: { startup: 'npm ci' },
    revision: 'r1',
  };
  const store: EnvironmentStore = {
    list: async () => ({ environments: [saved] }),
    load: async () => structuredClone(saved),
    remove: async () => {},
  };
  return { store };
}

test('environment edits capture one CAS snapshot and protect unsaved navigation', async (t) => {
  const { ui } = await editingFixture(t);
  await ui.input('Startup script', 'npm ci --ignore-scripts');
  await ui.command({});
  assert.equal(ui.messages.at(-1).problem.code, 'unsaved_changes');
  const snapshot = await ui.requestSave();
  assert.deepEqual(snapshot.environment, {
    id: 'env-1',
    name: 'node-tools',
    definition: { startup: 'npm ci --ignore-scripts' },
    expectedRevision: 'r1',
  });
  assert.deepEqual(snapshot.authority, { userId: 'user', organizationId: 'org', scope: 'user' });
  assert.equal(ui.messages.at(-1).dirty, false);
  assert.equal(ui.button('Add variable').type, 'button');
});

test('a new environment adopts its server identity and saved revision', async (t) => {
  const { store } = fixture();
  const ui = await renderWorkspace(t, store);
  await ui.click('New environment');
  await ui.input('Environment name', 'fresh-tools');
  await ui.input('Startup script', 'npm ci');
  const snapshot = await ui.requestSave();
  assert.deepEqual(snapshot.environment, {
    name: 'fresh-tools',
    definition: { startup: 'npm ci' },
    expectedRevision: null,
  });
  assert.equal(ui.messages.at(-1).dirty, false);
  assert.equal(ui.label('Select environment').value, 'env-2');
  const next = await ui.requestSave(false);
  assert.equal(next.environment.id, 'env-2');
  assert.equal(next.environment.expectedRevision, 'r2');
});

test('focused pending variable names are committed before capturing the environment save', async (t) => {
  const { ui } = await editingFixture(t);
  await ui.click('Add variable');
  await act(async () => ui.label('Variable VARIABLE name').focus());
  await ui.input('Variable VARIABLE name', 'PROJECT_MODE');
  const snapshot = await ui.requestSave();
  assert.deepEqual(snapshot.environment.definition.variables, { PROJECT_MODE: '' });
});

test('failed save and deletion retain the environment draft and saved revision', async (t) => {
  const { store } = fixture();
  store.remove = async () => {
    throw new Error('This environment was changed in another tab.');
  };
  const ui = await renderWorkspace(t, store);
  await ui.input('Select environment', 'env-1', 'change');
  await ui.input('Startup script', 'local unsaved work');
  const snapshot = await ui.requestSave(false);
  await ui.acknowledge(snapshot, {
    problem: { code: 'environment_conflict', message: 'Reload the page and try again.' },
  });
  assert.equal(ui.label('Startup script').value, 'local unsaved work');
  assert.equal(
    ui.messages.filter((message) => message.problem?.code === 'environment_conflict').at(-1)?.problem
      .code,
    'environment_conflict'
  );
  assert.throws(() => ui.button('Reload environment'));
  await ui.click('Delete environment');
  assert.match(ui.dom.window.document.body.textContent!, /changed in another tab/);
  assert.equal(ui.label('Startup script').value, 'local unsaved work');
  await ui.command({ discard: true });
  assert.equal(ui.label('Startup script').value, 'npm ci');
});

test('read-only environment view can select resources but cannot modify or delete them', async (t) => {
  const { store } = fixture();
  const ui = await renderWorkspace(t, store, true);
  await ui.input('Select environment', 'env-1', 'change');
  assert.equal(ui.dom.window.document.querySelector('fieldset')!.disabled, true);
  const text = ui.dom.window.document.body.textContent!;
  assert.doesNotMatch(text, /New environment|Save environment|Reload environment|Delete environment/);
  assert.equal(ui.label('Startup script').value, 'npm ci');
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
    environmentApi: '/ui/api/environments',
  });
  assert.ok(host.configuration, 'The real workspace bridge must accept host initialization');
  return { host, receive };
}

test('remount restores committed edits and their original CAS revision', async (t) => {
  const { ui } = await editingFixture(t);
  await ui.input('Startup script', 'preserve this edited startup');
  await ui.remount();
  assert.equal(ui.label('Startup script').value, 'preserve this edited startup');
  const snapshot = await ui.requestSave();
  assert.equal(snapshot.environment.expectedRevision, 'r1');
  assert.equal(snapshot.environment.definition.startup, 'preserve this edited startup');
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
  const { store } = fixture();
  const ui = await renderWorkspace(t, store);
  await ui.input('Select environment', 'env-1', 'change');
  return { ui };
}

test('large escaped environment definitions recover with their CAS base within the shared bound', async (t) => {
  const { store } = fixture();
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
  const saved = await ui.requestSave();
  assert.equal(saved.environment.expectedRevision, 'large-r1');
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

test('hosted collection contexts recover only their own drafts', async (t) => {
  const { store } = fixture();
  const ui = await renderWorkspace(t, store);
  await ui.remount('test', '?context=first');
  await ui.input('Select environment', 'env-1', 'change');
  await ui.input('Startup script', 'first repository draft');
  await ui.remount('test', '?context=second');
  assert.equal(ui.dom.window.document.querySelector('textarea'), null);
  await ui.remount('test', '?context=first');
  assert.equal(ui.label('Startup script').value, 'first repository draft');
});

test('switching environments discards pending field edits only after confirmation', async (t) => {
  const { ui } = await pendingNameFixture(t);
  await ui.input('Select environment', 'env-2', 'change');
  assert.equal(ui.label('Variable MODE name').value, 'MODE');
  assert.equal(ui.messages.at(-1).pending, false);
  assert.equal(ui.messages.at(-1).dirty, false);
});

test('a save acknowledgement settles only its captured environment snapshot', async (t) => {
  const { ui } = await editingFixture(t);
  await ui.input('Startup script', 'captured');
  const snapshot = await ui.requestSave(false);
  await ui.input('Startup script', 'newer edit');
  await ui.acknowledge(snapshot, {
    environment: {
      id: 'env-1',
      name: 'node-tools',
      definition: { startup: 'captured' },
      revision: 'r2',
    },
  });
  assert.equal(ui.label('Startup script').value, 'newer edit');
  assert.equal(ui.messages.filter((message) => message.type === 'state').at(-1)?.dirty, true);
  const next = await ui.requestSave(false);
  assert.equal(next.environment.expectedRevision, 'r2');
});

test('host-owned settings participate in navigation protection without entering the snapshot', async (t) => {
  const { ui } = await editingFixture(t);
  await ui.send({ type: 'environment_settings', dirty: true });
  assert.equal(ui.messages.filter((message) => message.type === 'state').at(-1)?.dirty, true);
  ui.dom.window.confirm = () => false;
  await ui.input('Select environment', 'env-1', 'change');
  assert.equal(ui.label('Startup script').value, 'npm ci');
  const snapshot = await ui.requestSave(false);
  assert.equal(Object.hasOwn(snapshot.environment, 'hostMetadata'), false);
});

test('a save acknowledgement preserves host settings changed after capture', async (t) => {
  const { ui } = await editingFixture(t);
  await ui.send({ type: 'environment_settings', dirty: true });
  const snapshot = await ui.requestSave(false);
  await ui.send({ type: 'environment_settings', dirty: true });
  await ui.acknowledge(snapshot, {
    environment: {
      id: 'env-1',
      name: 'node-tools',
      definition: { startup: 'npm ci' },
      revision: 'r2',
    },
  });
  assert.equal(ui.messages.filter((message) => message.type === 'state').at(-1)?.dirty, true);
});

async function pendingNameFixture(t: TestContext) {
  const { store } = fixture();
  const environments = ['env-1', 'env-2'].map((id) => ({
    id,
    name: id,
    revision: 'r1',
    definition: { variables: { MODE: 'test' } },
  }));
  store.list = async () => ({ environments });
  store.load = async (id) => structuredClone(environments.find((item) => item.id === id)!);
  const ui = await renderWorkspace(t, store);
  await ui.input('Select environment', 'env-1', 'change');
  await act(async () => ui.label('Variable MODE name').focus());
  await ui.input('Variable MODE name', 'bad name');
  await act(async () => ui.label('Variable MODE name').blur());
  assert.equal(ui.messages.at(-1).pending, true);
  return { store, ui };
}
