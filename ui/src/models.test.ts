import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ModelPicker } from './ModelPicker';
import {
  documentModels,
  modelGroups,
  recentModels,
  rememberModels,
  type ModelGroup,
} from './models';
import type { Runtime } from './domain';

const runtime = (nodes: Runtime['nodes']): Runtime => ({
  harness: 'codex',
  provider: 'openai',
  size: 'small',
  nodes,
});

test('document models are the distinct non-empty model IDs already in the runtime, in order', () => {
  const nodes = {
    plan: { kind: 'agent', model: 'private/model:b' },
    review: { kind: 'agent', model: 'model-a' },
    build: { kind: 'agent', model: 'private/model:b' },
    draft: { kind: 'agent', model: '' },
    blank: { kind: 'agent', model: '  ' },
    unset: { kind: 'agent' },
    deliver: { kind: 'git_delivery', connections: { github: ['GH_TOKEN'] } },
    future: { kind: 'future_worker', model: 'opaque-future' },
  };
  assert.deepEqual(documentModels(runtime(nodes)), ['private/model:b', 'model-a', 'opaque-future']);
  assert.deepEqual(documentModels(runtime({})), [], 'there is no catalog to fall back to');
});

test('suggestions list profile models, then recent ones, never the value already shown', () => {
  const doc = runtime({
    plan: { kind: 'agent', model: 'model-a' },
    build: { kind: 'agent', model: 'model-typed' },
    review: { kind: 'agent', model: 'model-a' },
  });
  assert.deepEqual(modelGroups(doc, ['model-old', 'model-a'], 'model-typed'), [
    { label: 'In this profile', models: ['model-a'] },
    { label: 'Recently used', models: ['model-old'] },
  ]);
  assert.deepEqual(
    modelGroups(doc, ['model-a', 'model-saved'], 'model-a'),
    [
      { label: 'In this profile', models: ['model-typed'] },
      { label: 'Recently used', models: ['model-saved'] },
    ],
    'a field shared with other agents or saved before still hides its own value'
  );
  assert.deepEqual(modelGroups(runtime({}), ['model-saved'], 'model-saved'), []);
  assert.deepEqual(
    modelGroups(doc, [], ''),
    [{ label: 'In this profile', models: ['model-a', 'model-typed'] }],
    'an empty field offers every model already bound'
  );
});

function installStorage(t: TestContext) {
  const values = new Map<string, string>();
  const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
    },
  });
  t.after(() => {
    if (original) Object.defineProperty(globalThis, 'localStorage', original);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  });
  return values;
}

test('recent models put the latest saves first, stay bounded, and tolerate bad storage', (t) => {
  assert.deepEqual(recentModels('missing-storage'), [], 'no storage means no suggestions');
  const values = installStorage(t);
  rememberModels('recent', ['model-a', 'model-b']);
  rememberModels('recent', ['model-c', 'model-a']);
  assert.deepEqual(recentModels('recent'), ['model-c', 'model-a', 'model-b']);
  rememberModels('recent', []);
  assert.deepEqual(recentModels('recent'), ['model-c', 'model-a', 'model-b']);
  rememberModels(
    'recent',
    Array.from({ length: 12 }, (_, i) => `model-${i}`)
  );
  assert.equal(recentModels('recent').length, 10);
  values.set('broken', '{');
  assert.deepEqual(recentModels('broken'), []);
  values.set('odd', JSON.stringify(['model-a', 3, '', null]));
  assert.deepEqual(recentModels('odd'), ['model-a']);
});

const picker = (value: string, suggestions: ModelGroup[], extra: Record<string, unknown> = {}) =>
  renderToStaticMarkup(
    createElement(ModelPicker, {
      label: 'Model',
      value,
      suggestions,
      harness: 'codex',
      provider: 'openai',
      onChange: () => assert.fail('rendering must not change the model'),
      ...extra,
    })
  );

test('the model picker is the same labelled text field with or without suggestions', () => {
  for (const suggestions of [[], [{ label: 'In this profile', models: ['model-a'] }]]) {
    const markup = picker('private/model-release:custom', suggestions, { invalid: true });
    assert.equal(markup.includes('<select'), false);
    assert.match(markup, /<label for="([^"]+)">Model<\/label><div[^>]*><input id="\1"/);
    assert.match(markup, /role="combobox"/);
    assert.match(markup, /value="private\/model-release:custom"/);
    assert.equal(markup.match(/aria-invalid="true"/g)?.length, 1);
    assert.match(markup, /aria-expanded="false"/);
    assert.equal(markup.includes('role="listbox"'), false, 'the list opens on demand');
    assert.equal(markup.includes('model-toggle'), suggestions.length > 0);
  }
});

test('missing runtime settings point to the Runtime dialog only when it can be opened', () => {
  const hint = picker('', [], { harness: '', provider: '', openRuntime: () => {} });
  assert.match(hint, /Choose a harness and provider in/);
  assert.match(hint, /aria-describedby="[^"]+-runtime-hint"/);
  assert.equal(picker('', [], { provider: '' }).includes('Runtime settings'), false);
});
