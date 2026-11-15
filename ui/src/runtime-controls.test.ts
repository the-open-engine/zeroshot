import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement, isValidElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ModelPicker } from './ModelPicker';
import { RuntimeEditor } from './RuntimeEditor';
import { NumericDraftProvider, createNumericDrafts } from './numeric-drafts';
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
  const drafts = createNumericDrafts();
  try {
    renderToStaticMarkup(
      createElement(NumericDraftProvider, { value: drafts }, createElement(Probe))
    );
  } finally {
    drafts.clear();
  }
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
          harnessConfig: { args: ['--tools', 'Read,Grep,Glob'] },
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
  assert.deepEqual(
    edits[0].runtime.nodes.worker.harnessConfig,
    before.runtime.nodes.worker.harnessConfig
  );
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

test('different effective lanes keep separate model fields even when their IDs agree', () => {
  const doc = document('codex', 'openai');
  doc.runtime.nodes.reviewer.model = doc.runtime.nodes.worker.model;
  doc.runtime.nodes.reviewer.lane = { harness: 'claude', provider: 'openrouter' };
  doc.runtime.nodes.worker.lane = { harness: 'codex', provider: 'openrouter' };
  const before = structuredClone(doc);
  const edits: Document[] = [];
  let reviewer: ReactElement<any> | undefined;
  within(
    () =>
      RuntimeEditor({
        document: doc,
        schema: {},
        recentModels: (harness, provider) => [`${harness}/${provider}/recent`],
        edit: (next) => edits.push(next),
      }),
    (tree) => {
      const pickers = tree.filter((element) => element.type === ModelPicker);
      assert.equal(pickers.length, 2);
      assert.equal(
        tree.some((element) => element.props.children === 'Use one model for all'),
        false
      );
      reviewer = pickers.find((picker) => picker.props.label === 'Model for reviewer');
      assert.deepEqual(
        [reviewer!.props.harness, reviewer!.props.provider],
        ['claude', 'openrouter']
      );
      assert.deepEqual(reviewer!.props.suggestions, [
        { label: 'Recently used', models: ['claude/openrouter/recent'] },
      ]);
      const worker = pickers.find((picker) => picker.props.label === 'Model for worker')!;
      assert.deepEqual([worker.props.harness, worker.props.provider], ['codex', 'openrouter']);
      assert.deepEqual(worker.props.suggestions, [
        { label: 'Recently used', models: ['codex/openrouter/recent'] },
      ]);
    }
  );
  reviewer!.props.onChange('provider-owned/new-id');
  assert.equal(edits[0].runtime.nodes.reviewer.model, 'provider-owned/new-id');
  assert.deepEqual(edits[0].runtime.nodes.reviewer.lane, before.runtime.nodes.reviewer.lane);
  assert.deepEqual(edits[0].runtime.nodes.worker, before.runtime.nodes.worker);
  assert.deepEqual(doc, before);
});

test('a shared picker uses the common override rather than the graph default', () => {
  const doc = document('codex', 'openai');
  for (const name of ['worker', 'reviewer']) {
    doc.runtime.nodes[name].model = 'common-id';
    doc.runtime.nodes[name].lane = { harness: 'claude', provider: 'anthropic' };
  }
  within(
    () =>
      RuntimeEditor({
        document: doc,
        schema: {},
        edit: () => {},
        recentModels: (harness, provider) => {
          assert.deepEqual([harness, provider], ['claude', 'anthropic']);
          return ['native-subscription-model'];
        },
      }),
    (tree) => {
      const pickers = tree.filter((element) => element.type === ModelPicker);
      assert.equal(pickers.length, 1);
      assert.equal(pickers[0].props.label, 'Model for all agents');
      assert.deepEqual(
        [pickers[0].props.harness, pickers[0].props.provider],
        ['claude', 'anthropic']
      );
    }
  );
});

test('changing the harness retains native configuration for review without translation', () => {
  const doc = document('claude', 'anthropic');
  const before = structuredClone(doc.runtime.nodes.worker.harnessConfig);
  const edits: Document[] = [];
  let harness: ReactElement<any> | undefined;
  within(
    () => RuntimeEditor({ document: doc, schema: {}, edit: (next) => edits.push(next) }),
    (tree) => {
      harness = tree.find(
        (element) => element.type === 'select' && element.props.value === 'claude'
      );
    }
  );
  assert.ok(harness);
  harness.props.onChange({ target: { value: 'copilot' } });
  assert.equal(edits[0].runtime.harness, 'copilot');
  assert.deepEqual(edits[0].runtime.nodes.worker.harnessConfig, before);
  assert.equal(doc.runtime.harness, 'claude');
});
