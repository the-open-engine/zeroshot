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
    getComputedStyle: dom.window.getComputedStyle.bind(dom.window),
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

test('mounted Runtime dialog edits a draft shared by its Form and JSON views', async (t) => {
  let unmount: (() => Promise<void>) | undefined;
  t.after(async () => unmount?.());
  const browser = installBrowser(t);
  const server = await createViteTestServer(t);
  const { App } = await server.ssrLoadModule('/src/App.tsx');
  const { createRoot } = await import('react-dom/client');
  const agents = (): Document => ({
    ...profile(),
    graph: {
      ...profile().graph,
      root: {
        kind: 'seq',
        name: 'run',
        state: { kind: 'record', fields: {} },
        children: [
          { kind: 'step', name: 'plan', worker: 'agent.worker@1' },
          { kind: 'step', name: 'build', worker: 'agent.worker@1' },
        ],
      },
    },
    runtime: {
      harness: 'codex',
      provider: 'openai',
      size: 'small',
      nodes: {
        plan: { kind: 'agent', model: 'model-a', effort: 'high' },
        build: { kind: 'agent', model: 'model-b' },
      },
    },
  });
  const bootstrap: Bootstrap = {
    version: 1,
    workspace: { kind: 'local', id: 'runtime-dialog-test' },
    templates: [],
    runtimeSchema: {
      oneOf: [
        { properties: { harness: { const: 'codex' }, provider: { enum: ['openai', 'gateway'] } } },
      ],
      $defs: { RunSize: { enum: ['small', 'large'] } },
    },
  };
  const services: WorkspaceServices = {
    mount: new URL('https://example.test/ui/'),
    catalog: async () => bootstrap,
    profiles: {
      list: async () => ({ profiles: [{ id: 'profile-1', name: 'runtime', isDefault: false }] }),
      load: async () => ({
        profile: { ...agents(), id: 'profile-1', isDefault: false },
        revision: 'r1',
      }),
      save: async () => {
        throw new Error('Saving is not part of this test.');
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

  const dialog = () => container.querySelector<HTMLDialogElement>('dialog[aria-label="Runtime"]');
  const confirmDialog = () =>
    container.querySelector<HTMLDialogElement>('dialog[aria-label="Discard runtime changes?"]');
  const button = (scope: ParentNode, text: string) => {
    const found = [...scope.querySelectorAll<HTMLButtonElement>('button')].find(
      (candidate) => candidate.textContent?.trim() === text
    );
    assert.ok(found, `button ${text}`);
    return found;
  };
  const click = (element: HTMLElement) => act(async () => element.click());
  const type = (element: HTMLInputElement | HTMLTextAreaElement, value: string) =>
    act(async () => {
      const prototype =
        element instanceof browser.HTMLTextAreaElement
          ? browser.HTMLTextAreaElement.prototype
          : browser.HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(prototype, 'value')!.set!.call(element, value);
      element.dispatchEvent(new browser.Event('input', { bubbles: true }));
    });
  const choose = (select: HTMLSelectElement, text: string) =>
    act(async () => {
      const option = [...select.options].find((candidate) => candidate.text === text);
      assert.ok(option, `option ${text}`);
      select.value = option.value;
      select.dispatchEvent(new browser.Event('change', { bubbles: true }));
    });
  const labelled = (label: string) => {
    const element = [...dialog()!.querySelectorAll('label')].find(
      (candidate) => candidate.textContent === label
    );
    assert.ok(element, `label ${label}`);
    return dialog()!.querySelector<HTMLSelectElement>(`[id="${element.htmlFor}"]`)!;
  };
  const model = (label: string) => labelled(label) as unknown as HTMLInputElement;
  const options = () =>
    [...dialog()!.querySelectorAll('[role="option"]')].map((option) => option.textContent);
  const key = (element: HTMLElement, name: string) =>
    act(async () => {
      element.dispatchEvent(
        new browser.KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true })
      );
    });
  const providerSelect = () =>
    [...dialog()!.querySelectorAll('select')].find((select) =>
      [...select.options].some((option) => option.value === 'gateway')
    )!;
  const undo = () => container.querySelector<HTMLButtonElement>('[aria-label="Undo"]')!;
  const openRuntime = () =>
    click(container.querySelector<HTMLButtonElement>('[aria-label="Runtime settings"]')!);
  const runtimeJson = async () => {
    await click(button(dialog()!, 'JSON'));
    const editor = dialog()!.querySelector<HTMLTextAreaElement>('[aria-label="JSON source"]');
    assert.ok(editor);
    return { editor, value: JSON.parse(editor.value) };
  };

  // Closing without changes is immediate.
  await openRuntime();
  assert.ok(dialog());
  assert.equal(button(dialog()!, 'Form').getAttribute('aria-pressed'), 'true');
  assert.equal(dialog()!.textContent?.includes('Runtime JSON'), false);
  await click(button(dialog()!, 'Cancel'));
  assert.equal(dialog(), null);
  assert.equal(confirmDialog(), null);

  // Form edits stay in the draft; closing asks before discarding them.
  await openRuntime();
  await choose(providerSelect(), 'gateway');
  assert.equal(undo().disabled, true, 'the document is unchanged until Apply changes');
  await click(dialog()!.querySelector<HTMLButtonElement>('[aria-label="Close dialog"]')!);
  assert.ok(confirmDialog());
  await click(button(confirmDialog()!, 'Keep editing'));
  assert.equal(providerSelect().value, 'gateway');
  await act(async () => {
    dialog()!.dispatchEvent(new browser.Event('cancel', { cancelable: true }));
  });
  assert.ok(confirmDialog(), 'Escape asks before discarding the draft');
  await click(button(confirmDialog()!, 'Discard'));
  assert.equal(dialog(), null);
  assert.equal(undo().disabled, true);
  await openRuntime();
  assert.equal(providerSelect().value, 'openai', 'discarded drafts do not return');

  // The draft crosses Form -> JSON -> Form, and invalid JSON keeps the JSON view.
  await choose(providerSelect(), 'gateway');
  const { editor, value } = await runtimeJson();
  assert.equal(value.provider, 'gateway');
  assert.equal(button(dialog()!, 'JSON').getAttribute('aria-pressed'), 'true');
  await type(editor, '{');
  await click(button(dialog()!, 'Form'));
  assert.ok(dialog()!.querySelector('.modal-error'));
  assert.ok(dialog()!.querySelector('[aria-label="JSON source"]'));
  value.nodes.plan.model = 'typed-in-json';
  await type(editor, JSON.stringify(value));
  await click(button(dialog()!, 'Form'));
  assert.equal(dialog()!.querySelector('[aria-label="JSON source"]'), null);
  assert.equal(dialog()!.querySelector('.modal-error'), null);
  assert.equal(providerSelect().value, 'gateway');
  assert.equal(
    model('Model for plan').value,
    'typed-in-json',
    'agents with different models open with per-agent choices'
  );

  // Suggestions follow the draft, and every value stays free text.
  const build = model('Model for build');
  await act(async () => build.focus());
  assert.equal(build.getAttribute('aria-expanded'), 'true');
  assert.deepEqual(options(), ['typed-in-json'], 'build is offered the model just typed for plan');
  await key(build, 'Escape');
  assert.equal(build.getAttribute('aria-expanded'), 'false');
  assert.ok(dialog(), 'Escape closes only the suggestion list');
  assert.equal(confirmDialog(), null);
  const scrolled: string[] = [];
  browser.HTMLElement.prototype.scrollIntoView = function (this: HTMLElement) {
    scrolled.push(this.id);
  };
  await key(build, 'ArrowDown');
  assert.deepEqual(
    scrolled,
    [build.getAttribute('aria-activedescendant')],
    'the keyboard choice scrolls into view'
  );
  assert.equal(
    build.getAttribute('aria-activedescendant'),
    dialog()!.querySelector('[role="option"]')!.id
  );
  await key(build, 'Enter');
  assert.equal(build.value, 'typed-in-json');
  assert.equal(build.getAttribute('aria-expanded'), 'false');
  await type(build, 'typed');
  assert.deepEqual(options(), ['typed-in-json'], 'typing narrows the list');
  await type(build, 'private/model:z');
  assert.equal(build.value, 'private/model:z');
  assert.deepEqual(options(), []);

  // One model for every agent, then a single undoable apply.
  await click(button(dialog()!, 'Use one model for all'));
  const all = model('Model for all agents');
  assert.equal(all.value, '');
  assert.equal(all.placeholder, 'Different per agent');
  await type(all, 'model-b');
  assert.equal(button(dialog()!, 'Choose per agent').getAttribute('aria-expanded'), 'false');
  assert.equal(undo().disabled, true);
  await click(button(dialog()!, 'Apply changes'));
  assert.equal(dialog(), null);
  assert.equal(confirmDialog(), null);
  assert.match(container.textContent ?? '', /Unsaved/);
  await openRuntime();
  assert.equal(model('Model for all agents').value, 'model-b', 'a shared model opens as one field');
  const applied = (await runtimeJson()).value;
  assert.equal(applied.provider, 'gateway');
  assert.deepEqual(applied.nodes, {
    plan: { kind: 'agent', model: 'model-b', effort: 'high' },
    build: { kind: 'agent', model: 'model-b' },
  });
  await click(button(dialog()!, 'Cancel'));
  assert.equal(dialog(), null, 'switching views alone is not a change');
  await click(undo());
  assert.equal(undo().disabled, true, 'applying the dialog is one undoable edit');
  await openRuntime();
  assert.deepEqual((await runtimeJson()).value, agents().runtime);
  await unmount();
  unmount = undefined;
});

test('opening another profile closes the Runtime draft, and saves feed model suggestions', async (t) => {
  let unmount: (() => Promise<void>) | undefined;
  t.after(async () => unmount?.());
  const browser = installBrowser(t);
  const server = await createViteTestServer(t);
  const { App } = await server.ssrLoadModule('/src/App.tsx');
  const { workspaceStorageKeys } = await server.ssrLoadModule('/src/workspace-storage.ts');
  const { createRoot } = await import('react-dom/client');
  const named = (name: string, model: string): Document => ({
    ...profile(),
    name,
    graph: {
      ...profile().graph,
      root: {
        kind: 'seq',
        name: 'run',
        state: { kind: 'record', fields: {} },
        children: [{ kind: 'step', name: 'plan', worker: 'agent.worker@1' }],
      },
    },
    runtime: { ...profile().runtime, nodes: { plan: { kind: 'agent', model } } },
  });
  const stored: Record<string, Document> = {
    first: named('first', 'model-a'),
    second: named('second', 'model-z'),
  };
  const bootstrap: Bootstrap = {
    version: 1,
    workspace: { kind: 'local', id: 'runtime-switch-test' },
    templates: [],
    runtimeSchema: {},
  };
  const services: WorkspaceServices = {
    mount: new URL('https://example.test/ui/'),
    catalog: async () => bootstrap,
    profiles: {
      list: async () => ({
        profiles: Object.keys(stored).map((name) => ({ id: name, name, isDefault: false })),
      }),
      load: async (name) => ({
        profile: { ...stored[name], id: name, isDefault: false },
        revision: 'r1',
      }),
      save: async (next) => {
        stored[next.name] = { name: next.name, graph: next.graph, runtime: next.runtime };
        return {
          profile: { ...stored[next.name], id: next.name, isDefault: false },
          revision: 'r2',
        };
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
  const key = workspaceStorageKeys(services.mount, bootstrap.workspace).recentModels(
    'codex',
    'openai'
  );
  localStorage.setItem(key, JSON.stringify(['model-old']));
  const settle = () => act(async () => new Promise<void>((resolve) => setImmediate(resolve)));
  await act(async () => root.render(createElement(App, { services, bootstrap })));
  await settle();

  const dialog = () => container.querySelector<HTMLDialogElement>('dialog[aria-label="Runtime"]');
  const button = (text: string) => {
    const found = [...container.querySelectorAll<HTMLButtonElement>('button')].find(
      (candidate) => candidate.textContent?.trim() === text
    );
    assert.ok(found, `button ${text}`);
    return found;
  };
  const click = async (element: HTMLElement) => {
    await act(async () => element.click());
    await settle();
  };
  const field = () => dialog()!.querySelector<HTMLInputElement>('input[role="combobox"]')!;
  const openRuntime = () =>
    click(container.querySelector<HTMLButtonElement>('[aria-label="Runtime settings"]')!);

  await click(button('first'));
  await openRuntime();
  assert.equal(field().value, 'model-a');
  await click(button('second'));
  assert.equal(dialog(), null, 'a draft from the first profile cannot be applied to the second');
  await openRuntime();
  assert.equal(field().value, 'model-z');

  await act(async () => {
    Object.getOwnPropertyDescriptor(browser.HTMLInputElement.prototype, 'value')!.set!.call(
      field(),
      'model-new'
    );
    field().dispatchEvent(new browser.Event('input', { bubbles: true }));
  });
  await click(button('Apply changes'));
  const until = async (pattern: RegExp) => {
    for (let tick = 0; tick < 100 && !pattern.test(container.textContent ?? ''); tick++)
      await act(async () => new Promise<void>((resolve) => setTimeout(resolve, 20)));
    assert.match(container.textContent ?? '', pattern);
  };
  await until(/Profile valid/);
  await click(button('Save profile'));
  await until(/Profile saved/);
  assert.equal(stored.second.runtime.nodes.plan.model, 'model-new');
  assert.deepEqual(JSON.parse(localStorage.getItem(key)!), ['model-new', 'model-old']);

  await click(button('first'));
  await openRuntime();
  await act(async () => field().focus());
  assert.deepEqual(
    [...dialog()!.querySelectorAll('[role="listbox"] [role="group"]')].map((group) => [
      group.getAttribute('aria-label'),
      [...group.querySelectorAll('[role="option"]')].map((option) => option.textContent),
    ]),
    [['Recently used', ['model-new', 'model-old']]]
  );
  await act(async () => {
    field().dispatchEvent(
      new browser.KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true, cancelable: true })
    );
  });
  assert.equal(
    field().getAttribute('aria-activedescendant'),
    [...dialog()!.querySelectorAll('[role="option"]')].at(-1)!.id,
    'ArrowUp from an unhighlighted list starts at the last option'
  );
  await unmount();
  unmount = undefined;
});
