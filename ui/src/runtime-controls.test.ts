import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement, isValidElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ModelPicker } from './ModelPicker';
import { RuntimeEditor } from './RuntimeEditor';
import { assertDocument, type Document } from './domain';

function elements(value: ReactNode): ReactElement<any>[] {
  if (Array.isArray(value)) return value.flatMap(elements);
  if (!isValidElement<{ children?: ReactNode }>(value)) return [];
  return [value, ...elements(value.props.children)];
}

function document(harness: string, provider: string): Document {
  return {
    name: 'runtime-review',
    graph: {
      profile: 'full',
      initialInput: { kind: 'null' },
      policy: {},
      root: {
        kind: 'seq',
        name: 'run',
        children: [
          { kind: 'step', name: 'worker', worker: 'agent.worker@1' },
          { kind: 'verifier', name: 'delivery', worker: 'builtin.git-delivery.pr@1' },
        ],
      },
    },
    runtime: {
      harness,
      provider,
      size: 'small',
      nodes: {
        worker: {
          kind: 'agent',
          model: 'custom-original',
          effort: 'high',
          connections: { tools: ['TOKEN'] },
        },
        delivery: { kind: 'git_delivery', connections: { github: ['GH_TOKEN'] } },
      },
    },
  };
}

test('each new preset selection reaches the runtime binding through the rendered picker callback', () => {
  for (const [harness, provider, model] of [
    ['codex', 'openai', 'gpt-6-astra'],
    ['codex', 'openrouter', 'openai/gpt-6-astra'],
    ['codex', 'bedrock', 'openai.gpt-6-astra'],
    ['claude', 'anthropic', 'claude-fable-5-1'],
    ['claude', 'openrouter', 'anthropic/claude-fable-5.1'],
    ['claude', 'bedrock', 'global.anthropic.claude-fable-5-1'],
  ]) {
    const doc = document(harness, provider);
    const before = structuredClone(doc);
    const edits: Document[] = [];
    const view = RuntimeEditor({
      document: doc,
      schema: {},
      edit: (next) => edits.push(next),
      openJson: () => {},
    });
    const pickers = elements(view).filter((element) => element.type === ModelPicker);
    assert.equal(pickers.length, 1, 'delivery must not have a model picker');

    // Render establishes React's hook context; exercise the actual select callback and its
    // RuntimeEditor parent callback without a browser, network, or model invocation.
    function SelectPreset() {
      const picker = ModelPicker(pickers[0].props);
      const controls = elements(picker);
      assert.ok(
        controls.some((element) => element.type === 'option' && element.props.value === model)
      );
      const select = controls.find((element) => element.type === 'select')!;
      select.props.onChange({ target: { value: model } });
      return picker;
    }
    renderToStaticMarkup(createElement(SelectPreset));
    assert.equal(edits.length, 1);
    assert.equal(edits[0].runtime.nodes.worker.model, model);
    assert.deepEqual(
      edits[0].runtime.nodes.worker.connections,
      before.runtime.nodes.worker.connections
    );
    assert.equal(edits[0].runtime.nodes.worker.effort, 'high');
    assert.deepEqual(edits[0].runtime.nodes.delivery, before.runtime.nodes.delivery);
    assert.deepEqual(doc, before);
  }
});

test('runtime model editing does not offer implicit conversion of unknown or missing bindings', () => {
  for (const missing of [false, true]) {
    const doc = document('codex', 'openai');
    if (missing) delete doc.runtime.nodes.worker;
    else
      doc.runtime.nodes.worker = {
        kind: 'future_worker',
        model: 'opaque',
        settings: { version: 2 },
      };
    assertDocument(doc);
    const before = structuredClone(doc);
    const view = RuntimeEditor({
      document: doc,
      schema: {},
      edit: () => {},
      openJson: () => {},
    });
    assert.equal(elements(view).filter((element) => element.type === ModelPicker).length, 0);
    assert.deepEqual(doc, before);
  }
});
