import test from 'node:test';
import assert from 'node:assert/strict';
import { findNode, type Document, type GraphNode } from './domain';
import { parentStateInput, useParentStateInput } from './parent-state-input';

const field = (kind = 'string', required = true) => ({ type: { kind }, required });
const input = {
  kind: 'record',
  fields: {
    task: field(),
    city: field(),
    date: field(),
    attendees: field('integer'),
    budget: field('number'),
    accessibility: field(),
    venuePlan: field('string', false),
    reviewFeedback: field('string', false),
  },
};
const worker = (): GraphNode => ({
  kind: 'step',
  name: 'planner',
  worker: 'agent.planner@1',
  instructions: 'Plan the event.',
  input: { kind: 'null' },
  inputBindings: [],
  output: { kind: 'null' },
  writeBindings: [],
  attempts: 2,
  metadata: { retained: true },
});
function document(state: any = input, node = worker()): Document {
  return {
    name: 'parent-input-test',
    graph: {
      profile: 'openengine.graph.full/v1',
      initialInput: state,
      policy: {},
      root: { kind: 'seq', name: 'run', state, children: [node], promotedStatePaths: [] },
    },
    runtime: {
      harness: 'codex',
      provider: 'openai',
      size: 'small',
      nodes: {
        planner: { kind: 'agent', model: 'opaque-model', metadata: { preserved: true } },
      },
    },
  };
}

test('explicit parent-state input binds six required fields without requiring optional planning results', () => {
  const source = document(),
    before = structuredClone(source);
  const plan = parentStateInput(source, worker());
  assert.equal(plan.replacesExisting, false);
  assert.equal(plan.optionalFields, 2);
  assert.equal(plan.sourceName, 'run');
  const next = useParentStateInput(source, 'planner');
  const node = findNode(next.graph.root, 'planner')!;
  assert.deepEqual(node.input, input);
  assert.deepEqual(
    node.inputBindings.map((binding: any) => binding.target),
    [['task'], ['city'], ['date'], ['attendees'], ['budget'], ['accessibility']]
  );
  for (const binding of node.inputBindings)
    assert.deepEqual(binding.value, { source: 'state', path: binding.target });
  assert.deepEqual(next.runtime, source.runtime);
  assert.deepEqual({ ...node, input: worker().input, inputBindings: [] }, worker());
  assert.deepEqual(source, before);
  node.input.fields.task.required = false;
  assert.equal(source.graph.root.state.fields.task.required, true);
});

test('complete nested values avoid overlapping targets and preserve literal names and advanced metadata', () => {
  const state = JSON.parse(
    '{"kind":"record","metadata":{"version":3},"fields":{"a.b":{"type":{"kind":"record","fields":{"child":{"type":{"kind":"string"},"required":false}}},"required":true,"extra":"keep"},"rows":{"type":{"kind":"array","items":{"kind":"record","fields":{}}},"required":true},"__proto__":{"type":{"kind":"string"},"required":true}}}'
  );
  const plan = parentStateInput(document(state), worker());
  assert.deepEqual(plan.input, state);
  assert.deepEqual(
    plan.inputBindings.map((binding) => binding.target),
    [['a.b'], ['rows'], ['__proto__']]
  );
  assert.equal(plan.input.fields['a.b'].type.fields.child.required, false);
});

test('matching mapping metadata survives; replacing custom sources or old fields needs explicit review', () => {
  const node = worker();
  node.input = structuredClone(input);
  const matching = {
    target: ['task'],
    value: { source: 'state', path: ['task'], extra: 'selector' },
    extra: { binding: 2 },
  };
  node.inputBindings = [matching];
  assert.equal(parentStateInput(document(input, node), node).replacesExisting, false);
  assert.deepEqual(parentStateInput(document(input, node), node).inputBindings[0], matching);
  node.inputBindings.push({ target: ['city'], value: { source: 'item', path: ['city'] } });
  const plan = parentStateInput(document(input, node), node);
  assert.equal(plan.replacesExisting, true);
  assert.deepEqual(plan.inputBindings[1].value, { source: 'state', path: ['city'] });
  node.input = { kind: 'record', fields: { custom: field() }, metadata: 'old contract' };
  node.inputBindings = [];
  assert.equal(parentStateInput(document(input, node), node).replacesExisting, true);
});

test('absent required flags remain absent and null or empty state needs no bindings', () => {
  for (const state of [
    { kind: 'null' },
    { kind: 'record', fields: {} },
    { kind: 'record', fields: { optional: { type: { kind: 'string' } } } },
  ]) {
    const plan = parentStateInput(document(state), worker());
    assert.deepEqual(plan.input, state);
    assert.deepEqual(plan.inputBindings, []);
  }
});

test('root activities use initial input and nested activities use the nearest scope', () => {
  const source = document();
  source.graph.root = worker();
  assert.equal(parentStateInput(source, source.graph.root).sourceName, 'initial input');
  const state = { kind: 'record', fields: { local: field() } };
  source.graph.root = {
    kind: 'seq',
    name: 'outer',
    state: input,
    children: [{ kind: 'seq', name: 'inner', state, children: [worker()] }],
  };
  const plan = parentStateInput(source, worker());
  assert.equal(plan.sourceName, 'inner');
  assert.deepEqual(
    plan.inputBindings.map((binding) => binding.target),
    [['local']]
  );
});

test('map collection reads are not confused with full parent arrays, while ordinary map state is usable', () => {
  const state = {
    kind: 'record',
    fields: { rows: { type: { kind: 'array', items: { kind: 'string' } }, required: true } },
  };
  const source = document(state);
  source.graph.root.children = [
    {
      kind: 'map',
      name: 'batch',
      state,
      over: { source: 'state', path: ['rows'] },
      maxItems: 2,
      body: worker(),
      promotedStatePaths: [],
    },
  ];
  assert.equal(parentStateInput(source, worker()).inputBindings.length, 1);
  source.graph.root.children[0].promotedStatePaths = [['rows']];
  const before = structuredClone(source);
  assert.throws(() => useParentStateInput(source, 'planner'), /map collection/);
  assert.deepEqual(source, before);
});

test('fixed delivery, unsupported payloads and malformed mappings remain unchanged', () => {
  const source = document();
  source.runtime.nodes.planner = { kind: 'git_delivery' };
  assert.throws(() => useParentStateInput(source, 'planner'), /editable input contract/);
  assert.throws(
    () => useParentStateInput(document({ kind: 'string' }), 'planner'),
    /object or None/
  );
  const malformed = worker();
  malformed.inputBindings = { future: 'bindings' };
  assert.throws(
    () => useParentStateInput(document(input, malformed), 'planner'),
    /unsupported input mappings/
  );
  assert.throws(
    () => useParentStateInput(document({ kind: 'record', fields: { bad: null } }), 'planner'),
    /unsupported parent state fields/
  );
});
