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

/** Renders `view` inside React's hook context and hands its element tree to `inspect`. */
function within(view: () => ReactNode, inspect: (tree: ReactElement<any>[]) => void) {
  function Probe() {
    const tree = view();
    inspect(elements(tree));
    return tree;
  }
  renderToStaticMarkup(createElement(Probe));
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
          { kind: 'step', name: 'reviewer', worker: 'agent.worker@1' },
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
        reviewer: { kind: 'agent', model: 'reviewer-model' },
        delivery: { kind: 'git_delivery', connections: { github: ['GH_TOKEN'] } },
      },
    },
  };
}

test('a model chosen for one agent reaches only that runtime binding', () => {
  const doc = document('future-harness', 'private-provider');
  const before = structuredClone(doc);
  const edits: Document[] = [];
  let worker: ReactElement<any> | undefined;
  within(
    () => RuntimeEditor({ document: doc, schema: {}, edit: (next) => edits.push(next) }),
    (tree) => {
      const pickers = tree.filter((element) => element.type === ModelPicker);
      worker = pickers.find((picker) => picker.props.label === 'Model for worker');
      assert.equal(
        pickers.some((picker) => picker.props.label === 'Model for all agents'),
        false,
        'agents with different models start with per-agent choices'
      );
      assert.equal(
        pickers.some((picker) => picker.props.label === 'Model for delivery'),
        false,
        'delivery must not have a model picker'
      );
    }
  );
  assert.ok(worker);
  assert.deepEqual(worker.props.suggestions, [
    { label: 'In this profile', models: ['reviewer-model'] },
  ]);
  worker.props.onChange('reviewer-model');
  assert.equal(edits.length, 1);
  assert.equal(edits[0].runtime.nodes.worker.model, 'reviewer-model');
  assert.deepEqual(
    edits[0].runtime.nodes.worker.connections,
    before.runtime.nodes.worker.connections
  );
  assert.equal(edits[0].runtime.nodes.worker.effort, 'high');
  assert.deepEqual(edits[0].runtime.nodes.reviewer, before.runtime.nodes.reviewer);
  assert.deepEqual(edits[0].runtime.nodes.delivery, before.runtime.nodes.delivery);
  assert.deepEqual(doc, before);
});

test('agents sharing a model get one field that sets every agent at once', () => {
  for (const model of ['shared-model', '']) {
    const doc = document('codex', 'openai');
    doc.runtime.nodes.worker.model = model;
    doc.runtime.nodes.reviewer.model = model;
    const edits: Document[] = [];
    let labels: string[] = [];
    let shared: ReactElement<any> | undefined;
    within(
      () =>
        RuntimeEditor({
          document: doc,
          schema: {},
          recentModels: (harness, provider) => {
            assert.deepEqual([harness, provider], ['codex', 'openai']);
            return ['recent-model'];
          },
          edit: (next) => edits.push(next),
        }),
      (tree) => {
        const pickers = tree.filter((element) => element.type === ModelPicker);
        labels = pickers.map((picker) => picker.props.label);
        shared = pickers[0];
      }
    );
    assert.deepEqual(labels, ['Model for all agents'], 'a new or uniform profile hides the rest');
    assert.equal(shared!.props.value, model);
    assert.deepEqual(shared!.props.suggestions, [
      { label: 'Recently used', models: ['recent-model'] },
    ]);
    shared!.props.onChange('model-next');
    assert.equal(edits[0].runtime.nodes.worker.model, 'model-next');
    assert.equal(edits[0].runtime.nodes.reviewer.model, 'model-next');
    assert.equal(edits[0].runtime.nodes.worker.effort, 'high');
    assert.deepEqual(edits[0].runtime.nodes.delivery, doc.runtime.nodes.delivery);
  }
});

test('runtime model editing does not offer implicit conversion of unknown or missing bindings', () => {
  for (const missing of [false, true]) {
    const doc = document('codex', 'openai');
    delete doc.runtime.nodes.reviewer;
    if (missing) delete doc.runtime.nodes.worker;
    else
      doc.runtime.nodes.worker = {
        kind: 'future_worker',
        model: 'opaque',
        settings: { version: 2 },
      };
    assertDocument(doc);
    const before = structuredClone(doc);
    within(
      () => RuntimeEditor({ document: doc, schema: {}, edit: () => {} }),
      (tree) => assert.equal(tree.filter((element) => element.type === ModelPicker).length, 0)
    );
    assert.deepEqual(doc, before);
  }
});
