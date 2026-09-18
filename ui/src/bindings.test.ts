import test from 'node:test';
import assert from 'node:assert/strict';
import type { Document, GraphNode } from './domain';
import {
  appendMapping,
  bindingContext,
  bindingKey,
  dataChoices,
  fieldChoices,
  mapOverChoices,
  outputChoices,
  pathChoices,
  pathLabel,
  payloadAtPath,
  promotionChoices,
  removeMapping,
  selectorLabel,
  togglePromotion,
  updateMapping,
} from './bindings';

const string = { kind: 'string' };
const record = (fields: Record<string, any>) => ({
  kind: 'record',
  fields: Object.fromEntries(
    Object.entries(fields).map(([name, type]) => [name, { type, required: true }])
  ),
});
const array = (items: any) => ({ kind: 'array', items });
function document(root: GraphNode): Document {
  return {
    name: 'test',
    graph: { profile: 'test', root, initialInput: record({ task: string }), policy: {} },
    runtime: { harness: '', provider: '', size: 'small', nodes: {} },
  };
}
const values = (choices: { value: any }[]) => choices.map(({ value }) => value);

test('field paths keep literal dots distinct from nested segments and ignore inherited fields', () => {
  const fields = JSON.parse(
    '{"a.b":{"type":{"kind":"string"}},"__proto__":{"type":{"kind":"number"}}}'
  );
  fields.a = { type: record({ b: { kind: 'boolean' } }) };
  const schema = { kind: 'record', fields };
  assert.notEqual(bindingKey(['a.b']), bindingKey(['a', 'b']));
  assert.equal(pathLabel(['a.b']), 'a.b');
  assert.equal(pathLabel(['a', 'b']), 'a › b');
  assert.equal(payloadAtPath(schema, ['a.b'])?.kind, 'string');
  assert.equal(payloadAtPath(schema, ['a', 'b'])?.kind, 'boolean');
  assert.equal(payloadAtPath(schema, ['__proto__'])?.kind, 'number');
  assert.equal(payloadAtPath(schema, ['constructor']), undefined);
  assert.deepEqual(values(pathChoices(schema)), [['a.b'], ['__proto__'], ['a'], ['a', 'b']]);
});

test('schema inspection traverses record fields but never invents array index or whole-value paths', () => {
  const schema = record({
    rows: array(record({ id: string })),
    nested: record({ message: string }),
  });
  assert.deepEqual(
    fieldChoices(schema).map(({ path }) => path),
    [['rows'], ['nested'], ['nested', 'message']]
  );
  assert.equal(payloadAtPath(schema, ['rows', '0', 'id']), undefined);
  assert.equal(payloadAtPath(schema, []), undefined);
  assert.deepEqual(fieldChoices(array(string)), []);
  assert.deepEqual(fieldChoices({ kind: 'record', fields: { invalid: null } }), []);
  const cyclic: any = record({});
  cyclic.fields.self = { type: cyclic };
  assert.deepEqual(
    fieldChoices(cyclic).map(({ path }) => path),
    [['self']]
  );
});

test('software-change verifier offers its diagnostic message, verdict, and nearest feedback state', () => {
  const verifier: GraphNode = {
    kind: 'verifier',
    name: 'acceptance',
    input: record({ task: string }),
    output: { kind: 'null' },
    diagnostic: record({ message: string }),
    signals: { verdict: ['accepted', 'rejected'] },
    inputBindings: [{ target: ['task'], value: { source: 'state', path: ['task'] } }],
    writeBindings: [
      {
        target: ['acceptanceFeedback'],
        value: { node: 'acceptance', channel: 'diagnostic', path: ['message'] },
      },
    ],
  };
  const group: GraphNode = {
    kind: 'par',
    name: 'reviews',
    state: record({ task: string, acceptanceFeedback: string }),
    branches: [verifier],
  };
  const root: GraphNode = {
    kind: 'seq',
    name: 'run',
    state: record({ outer: string }),
    children: [group],
  };
  const context = bindingContext(document(root), verifier);
  assert.equal(context.state, group.state);
  assert.equal(context.stateName, 'reviews');
  assert.deepEqual(values(dataChoices(context)), [
    { source: 'state', path: ['task'] },
    { source: 'state', path: ['acceptanceFeedback'] },
  ]);
  assert.deepEqual(values(outputChoices(verifier)), [
    { node: 'acceptance', channel: 'diagnostic', path: ['message'] },
    { node: 'acceptance', channel: 'signal', path: ['verdict'] },
  ]);
  assert.ok(
    outputChoices(verifier).some(
      ({ value }) => bindingKey(value) === bindingKey(verifier.writeBindings[0].value)
    )
  );
});

test('map over reads its own state and map body receives the selected record item', () => {
  const worker: GraphNode = { kind: 'step', name: 'worker' };
  const inner: GraphNode = {
    kind: 'seq',
    name: 'inner',
    state: record({ feedback: string }),
    children: [worker],
  };
  const map: GraphNode = {
    kind: 'map',
    name: 'batch',
    state: record({ tasks: array(record({ title: string })), feedback: array(string) }),
    over: { source: 'state', path: ['tasks'] },
    body: inner,
  };
  const root: GraphNode = {
    kind: 'seq',
    name: 'run',
    state: record({ outerTasks: array(string) }),
    children: [map],
  };
  const doc = document(root);
  assert.deepEqual(values(mapOverChoices(map, bindingContext(doc, map))), [
    { source: 'state', path: ['tasks'] },
    { source: 'state', path: ['feedback'] },
  ]);
  const context = bindingContext(doc, worker);
  assert.equal(context.state, inner.state);
  assert.equal(context.itemName, 'batch');
  assert.deepEqual(values(dataChoices(context)), [
    { source: 'state', path: ['feedback'] },
    { source: 'item', path: ['title'] },
  ]);
});

test('nested maps can select an array field on the enclosing map item', () => {
  const worker: GraphNode = {
    kind: 'succeed',
    name: 'done',
    output: record({ result: string }),
    bindings: [],
  };
  const nested: GraphNode = {
    kind: 'map',
    name: 'nested',
    state: record({}),
    over: { source: 'item', path: ['tasks'] },
    body: worker,
  };
  const outer: GraphNode = {
    kind: 'map',
    name: 'outer',
    state: record({ batches: array(record({ tasks: array(record({ title: string })) })) }),
    over: { source: 'state', path: ['batches'] },
    body: nested,
  };
  const doc = document(outer);
  assert.deepEqual(values(mapOverChoices(nested, bindingContext(doc, nested))), [
    { source: 'item', path: ['tasks'] },
  ]);
  assert.deepEqual(values(dataChoices(bindingContext(doc, worker))), [
    { source: 'item', path: ['title'] },
  ]);
});

test('mapping edits preserve unknown selectors, metadata and literal paths until explicitly changed', () => {
  const original: GraphNode = {
    kind: 'verifier',
    name: 'review',
    custom: { keep: true },
    writeBindings: [
      {
        target: ['old.field'],
        value: { node: 'earlier', channel: 'diagnostic', path: ['unknown'] },
        custom: 'keep',
      },
      { target: ['second'], value: { node: 'review', channel: 'out', path: ['value'] } },
    ],
  };
  const before = structuredClone(original);
  const changed = updateMapping(original, 'writeBindings', 0, { target: ['feedback'] });
  assert.deepEqual(changed.writeBindings[0].value, original.writeBindings[0].value);
  assert.equal(changed.writeBindings[0].custom, 'keep');
  assert.equal(changed.writeBindings[1], original.writeBindings[1]);
  assert.equal(changed.custom, original.custom);
  const added = appendMapping(changed, 'writeBindings', ['new.field'], {
    node: 'review',
    channel: 'signal',
    path: ['verdict'],
  });
  assert.deepEqual(
    removeMapping(added, 'writeBindings', 1).writeBindings.map((binding: any) => binding.target),
    [['feedback'], ['new.field']]
  );
  assert.deepEqual(original, before);
});

test('promotion choices expose group fields, restrict map collection to arrays, and preserve unresolved paths', () => {
  const group: GraphNode = {
    kind: 'seq',
    name: 'run',
    state: record({ task: string, feedback: array(string) }),
    children: [],
    promotedStatePaths: [['removed.field']],
  };
  assert.deepEqual(values(promotionChoices(group)), [['task'], ['feedback']]);
  assert.deepEqual(values(promotionChoices({ ...group, kind: 'map' })), [['feedback']]);
  const selected = togglePromotion(group, ['feedback'], true);
  assert.deepEqual(selected.promotedStatePaths, [['removed.field'], ['feedback']]);
  assert.deepEqual(
    togglePromotion(selected, ['feedback'], true).promotedStatePaths,
    selected.promotedStatePaths
  );
  assert.deepEqual(togglePromotion(selected, ['removed.field'], false).promotedStatePaths, [
    ['feedback'],
  ]);
  assert.deepEqual(group.promotedStatePaths, [['removed.field']]);
});

test('schema changes affect choices without rewriting authored bindings or promotions', () => {
  const node: GraphNode = {
    kind: 'step',
    name: 'worker',
    input: record({ renamed: string }),
    inputBindings: [{ target: ['old'], value: { source: 'state', path: ['old'] } }],
  };
  const before = structuredClone(node);
  assert.deepEqual(values(pathChoices(node.input)), [['renamed']]);
  assert.deepEqual(outputChoices(node), []);
  assert.deepEqual(node, before);
});

test('unsupported lists and map selector sources remain opaque', () => {
  const node: GraphNode = {
    kind: 'step',
    name: 'worker',
    inputBindings: { future: true },
    promotedStatePaths: 'future',
  };
  assert.equal(updateMapping(node, 'inputBindings', 0, { target: ['task'] }), node);
  assert.equal(
    appendMapping(node, 'inputBindings', ['task'], { source: 'state', path: ['task'] }),
    node
  );
  assert.equal(removeMapping(node, 'inputBindings', 0), node);
  assert.equal(togglePromotion(node, ['task'], true), node);
  const map: GraphNode = {
    kind: 'map',
    name: 'map',
    state: record({ tasks: array(record({ title: string })) }),
    over: { source: 'future', path: ['tasks'] },
    body: node,
  };
  assert.equal(bindingContext(document(map), node).item, undefined);
});

test('unresolved labels never coerce malformed imported path segments or selector identities', () => {
  const malformed = JSON.parse(
    '{"target":[{"toString":null}],"value":{"source":"state","path":["task"]}}'
  );
  const before = structuredClone(malformed);
  assert.equal(pathLabel(malformed.target), 'Unresolved field');
  assert.equal(
    selectorLabel({ source: 'state', path: [{ toString: null }] }),
    'State · Unresolved field'
  );
  assert.equal(
    selectorLabel({ node: { toString: null }, channel: 'out', path: ['task'] }),
    'Unresolved source'
  );
  assert.equal(
    selectorLabel({ node: 'worker', channel: { toString: null }, path: ['task'] }),
    'Unresolved source'
  );
  assert.equal(typeof bindingKey(malformed.target), 'string');
  assert.deepEqual(malformed, before);
});
