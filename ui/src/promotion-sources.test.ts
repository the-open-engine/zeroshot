import test from 'node:test';
import assert from 'node:assert/strict';
import { allNodes, findNode, type Document, type GraphNode } from './domain';
import {
  connectPromotionSource,
  declaredSchemaFit,
  promotionTrace,
  promotionTraces,
} from './promotion-sources';

const text = { kind: 'string' };
const record = (fields: Record<string, any>, required = true) => ({
  kind: 'record',
  fields: Object.fromEntries(
    Object.entries(fields).map(([name, type]) => [name, { type, required }])
  ),
});
const state = () => record({ feedback: text });
const writer = (name = 'review'): GraphNode => ({
  kind: 'verifier',
  name,
  output: { kind: 'null' },
  diagnostic: record({ message: text }),
  signals: { verdict: ['accepted', 'rejected'] },
  inputBindings: [],
  writeBindings: [],
});
const seq = (name: string, children: GraphNode[], schema = state()): GraphNode => ({
  kind: 'seq',
  name,
  state: schema,
  children,
  promotedStatePaths: [],
});
function doc(group: GraphNode, schema = state()): Document {
  return {
    name: 'test',
    graph: {
      profile: 'test',
      initialInput: schema,
      policy: {},
      root: seq('root', [group], schema),
    },
    runtime: { harness: '', provider: '', size: 'small', nodes: {} },
  };
}
const diagnostic = { node: 'review', channel: 'diagnostic', path: ['message'] };

test('trace identifies a child source hidden by intermediate wrapper promotions', () => {
  const child = writer();
  child.writeBindings = [{ target: ['feedback'], value: diagnostic }];
  const body = seq('body', [child]);
  const loop: GraphNode = {
    kind: 'loop',
    name: 'retry',
    state: state(),
    body,
    promotedStatePaths: [['feedback']],
  };
  const document = doc(loop);
  const before = structuredClone(document);
  const trace = promotionTrace(document, loop, ['feedback']);
  const source = trace.sources.find((candidate) => candidate.existing)!;
  assert.deepEqual(source.chain, ['body', 'retry']);
  assert.deepEqual(source.missing, ['body']);
  assert.equal(source.connected, false);
  assert.equal(source.canConnect, true);
  const next = connectPromotionSource(document, loop, ['feedback'], source.id);
  assert.deepEqual(findNode(next, 'body')?.promotedStatePaths, [['feedback']]);
  assert.deepEqual(findNode(next, 'review')?.writeBindings, child.writeBindings);
  assert.deepEqual(document, before);
});

test('explicit source connection atomically adds the write and every wrapper promotion', () => {
  const child = writer();
  const inner = seq('inner', [child]);
  const group = seq('outer', [inner]);
  const document = doc(group);
  const source = promotionTrace(document, group, ['feedback']).sources.find(
    (candidate) => candidate.selector.channel === 'diagnostic'
  )!;
  assert.equal(source.existing, false);
  assert.equal(source.canConnect, true);
  const next = connectPromotionSource(document, group, ['feedback'], source.id);
  assert.deepEqual(findNode(next, 'review')?.writeBindings, [
    { target: ['feedback'], value: diagnostic },
  ]);
  assert.deepEqual(next.promotedStatePaths, [['feedback']]);
  assert.deepEqual(findNode(next, 'inner')?.promotedStatePaths, [['feedback']]);
  assert.deepEqual(child.writeBindings, []);
  assert.deepEqual(group.promotedStatePaths, []);
});

test('orphan loop field cannot pretend to return a value to a missing parent destination', () => {
  const loop: GraphNode = {
    kind: 'loop',
    name: 'retry',
    state: state(),
    body: seq('body', [writer()]),
    promotedStatePaths: [],
  };
  const trace = promotionTrace(doc(loop, record({ task: text })), loop, ['feedback']);
  assert.equal(trace.passThrough, false);
  assert.match(trace.issues.join(' '), /Add feedback to root state/);
  assert.equal(
    trace.sources.some((source) => source.canConnect),
    false
  );
});

test('missing wrapper field and producer/parent type mismatches block automatic wiring', () => {
  const child = writer();
  const group = seq('outer', [seq('inner', [child], record({ task: text }))]);
  const trace = promotionTrace(doc(group), group, ['feedback']);
  assert.ok(trace.sources.every((source) => !source.canConnect));
  assert.match(trace.sources[0].issues.join(' '), /Add feedback to inner state/);
  const typed = seq('typed', [writer()]);
  const mismatched = promotionTrace(doc(typed, record({ feedback: { kind: 'boolean' } })), typed, [
    'feedback',
  ]);
  assert.equal(mismatched.passThrough, false);
  assert.match(mismatched.schemaNote ?? '', /string; parent field accepts: boolean/);
  assert.match(
    mismatched.sources.find((source) => source.selector.channel === 'diagnostic')!.issues.join(' '),
    /different field type/
  );
  assert.equal(
    mismatched.sources.some((source) => source.canConnect),
    false
  );
});

test('map routes collect one item per child write and cannot pass through an incoming array', () => {
  const schema = record({ feedback: { kind: 'array', items: text } });
  const child = writer();
  const body = seq('body', [child], schema);
  const map: GraphNode = {
    kind: 'map',
    name: 'batch',
    state: schema,
    over: { source: 'state', path: ['feedback'] },
    body,
    promotedStatePaths: [],
  };
  const document = doc(map, schema);
  const trace = promotionTrace(document, map, ['feedback']);
  assert.equal(trace.passThrough, false);
  const source = trace.sources.find((candidate) => candidate.selector.channel === 'diagnostic')!;
  assert.equal(source.canConnect, true);
  const next = connectPromotionSource(document, map, ['feedback'], source.id);
  assert.deepEqual(next.promotedStatePaths, [['feedback']]);
  assert.deepEqual(findNode(next, 'body')?.promotedStatePaths, [['feedback']]);
  assert.equal(findNode(next, 'review')?.writeBindings[0].value.channel, 'diagnostic');
  const noWriter: GraphNode = { ...map, body: seq('empty', [], schema) };
  const empty = promotionTrace(doc(noWriter, schema), noWriter, ['feedback']);
  assert.equal(empty.passThrough, false);
  assert.deepEqual(empty.sources, []);
});

test('canonical-style required feedback passes through a choice without claiming a child result', () => {
  const schema = record({
    task: text,
    acceptanceFeedback: text,
    codeFeedback: text,
    deliveryFeedback: text,
  });
  const choice: GraphNode = {
    kind: 'choice',
    name: 'review_result',
    state: schema,
    branches: [
      {
        when: { kind: 'true' },
        node: { kind: 'succeed', name: 'done', output: { kind: 'null' }, bindings: [] },
      },
    ],
    promotedStatePaths: [['deliveryFeedback']],
  };
  const document = doc(choice, schema);
  const before = structuredClone(document);
  const trace = promotionTrace(document, choice, ['deliveryFeedback']);
  assert.equal(trace.selected, true);
  assert.equal(trace.passThrough, true);
  assert.deepEqual(trace.sources, []);
  assert.match(trace.passThroughNote, /not a child-produced result/);
  assert.deepEqual(document, before);
  const optional = { ...choice, state: record({ deliveryFeedback: text }, false) };
  assert.equal(
    promotionTrace(doc(optional, schema), optional, ['deliveryFeedback']).passThrough,
    false
  );
});

test('an authored write is never overwritten by connecting a new source', () => {
  const child = writer();
  child.writeBindings = [
    {
      target: ['feedback'],
      value: { node: 'earlier', channel: 'out', path: ['value'] },
      future: 'preserve',
    },
  ];
  const group = seq('group', [child]);
  const document = doc(group);
  const source = promotionTrace(document, group, ['feedback']).sources.find(
    (candidate) => !candidate.existing && candidate.selector.channel === 'diagnostic'
  )!;
  assert.equal(source.canConnect, false);
  assert.match(source.issues.join(' '), /already writes/);
  assert.throws(() => connectPromotionSource(document, group, ['feedback'], source.id));
  assert.equal(child.writeBindings[0].future, 'preserve');
});

test('a required inner field does not confirm pass-through from an optional parent field', () => {
  const group = seq('inner', []);
  group.promotedStatePaths = [['feedback']];
  const document = doc(group, record({ feedback: text }, false));
  const before = structuredClone(document);
  const trace = promotionTrace(document, group, ['feedback']);
  assert.equal(trace.passThrough, false);
  assert.equal(trace.selected, true);
  assert.deepEqual(trace.sources, []);
  assert.match(trace.passThroughNote, /parent must require this field/);
  assert.deepEqual(document, before);
});

test('parallel all conflicting producers are blocked; conditional routes are explicitly uncertain', () => {
  const a = writer('a'),
    b = writer('b');
  a.writeBindings = [
    { target: ['feedback'], value: { node: 'a', channel: 'diagnostic', path: ['message'] } },
  ];
  const parallel: GraphNode = {
    kind: 'par',
    name: 'reviews',
    join: { kind: 'all' },
    state: state(),
    branches: [a, b],
    promotedStatePaths: [],
  };
  const trace = promotionTrace(doc(parallel), parallel, ['feedback']);
  const conflicting = trace.sources.find(
    (source) => source.writer === 'b' && source.selector.channel === 'diagnostic'
  )!;
  assert.equal(conflicting.canConnect, false);
  assert.match(conflicting.issues.join(' '), /parallel branch/);
  const choice: GraphNode = {
    kind: 'choice',
    name: 'pick',
    state: state(),
    branches: [{ when: { kind: 'true' }, node: writer() }],
    promotedStatePaths: [],
  };
  const conditional = promotionTrace(doc(choice), choice, ['feedback']).sources.find(
    (source) => source.selector.channel === 'diagnostic'
  )!;
  assert.equal(conditional.canConnect, true);
  assert.match(conditional.notes.join(' '), /conditional branches/);
});

test('signals are enum-valued data, not automatically promoted group output or string fields', () => {
  const group = seq('group', [writer()]);
  assert.equal(
    promotionTrace(doc(group), group, ['feedback']).sources.find(
      (source) => source.selector.channel === 'signal'
    )?.canConnect,
    false
  );
  const schema = record({ feedback: { kind: 'enum', values: ['accepted', 'rejected'] } });
  const typed = seq('typed', [writer()], schema);
  assert.equal(
    promotionTrace(doc(typed, schema), typed, ['feedback']).sources.find(
      (source) => source.selector.channel === 'signal'
    )?.canConnect,
    true
  );
});

test('connecting a hidden authored route detects a visible parallel sibling without treating hidden writes as conflicts', () => {
  const a = writer('a'),
    b = writer('b');
  a.writeBindings = [
    { target: ['feedback'], value: { node: 'a', channel: 'diagnostic', path: ['message'] } },
  ];
  b.writeBindings = [
    { target: ['feedback'], value: { node: 'b', channel: 'diagnostic', path: ['message'] } },
  ];
  const left = seq('left', [a]),
    right = seq('right', [b]);
  right.promotedStatePaths = [['feedback']];
  const parallel: GraphNode = {
    kind: 'par',
    name: 'parallel',
    state: state(),
    branches: [left, right],
    join: { kind: 'all' },
    promotedStatePaths: [],
  };
  const document = doc(parallel);
  const source = promotionTrace(document, left, ['feedback']).sources.find(
    (candidate) => candidate.existing
  )!;
  assert.equal(source.canConnect, false);
  assert.match(source.issues.join(' '), /parallel branch/);
  right.promotedStatePaths = [];
  const isolated = promotionTrace(document, left, ['feedback']).sources.find(
    (candidate) => candidate.existing
  )!;
  assert.equal(isolated.canConnect, true);
});

test('literal paths and malformed existing promotions are preserved without coercion', () => {
  const schema = record({ 'a.b': text });
  const child = writer();
  const group = seq('group', [child], schema);
  group.promotedStatePaths = [[{ toString: null }]];
  const traces = promotionTraces(doc(group, schema), group);
  assert.equal(traces.length, 2);
  assert.equal(traces[1].label, 'Unresolved field');
  const source = traces[0].sources.find(
    (candidate) => candidate.selector.channel === 'diagnostic'
  )!;
  const next = connectPromotionSource(doc(group, schema), group, ['a.b'], source.id);
  assert.deepEqual(findNode(next, 'review')?.writeBindings[0].target, ['a.b']);
  assert.deepEqual(next.promotedStatePaths[0], [{ toString: null }]);
  assert.equal(allNodes(next).length, 2);
});

test('schema assistance leaves advanced record subtyping to Rust', () => {
  assert.equal(declaredSchemaFit({ kind: 'integer' }, { kind: 'number' }), 'match');
  assert.equal(declaredSchemaFit({ kind: 'number' }, { kind: 'string' }), 'mismatch');
  assert.equal(declaredSchemaFit(record({ a: text, b: text }), record({ a: text })), 'unknown');
  assert.equal(declaredSchemaFit(record({ a: text }), record({ a: text })), 'match');
});

test('a structurally connected optional signal route still explains producer failure outcomes', () => {
  const schema = record({ feedback: { kind: 'enum', values: ['accepted', 'rejected'] } }, false);
  const child = writer();
  const body = seq('body', [child], schema);
  const loop: GraphNode = {
    kind: 'loop',
    name: 'loop',
    state: schema,
    body,
    maxIterations: 1,
    promotedStatePaths: [],
  };
  const document = doc(loop, schema);
  const source = promotionTrace(document, loop, ['feedback']).sources.find(
    (candidate) => candidate.selector.channel === 'signal'
  )!;
  const connected = connectPromotionSource(document, loop, ['feedback'], source.id);
  const next = doc(connected, schema);
  const trace = promotionTrace(next, connected, ['feedback']);
  assert.equal(trace.passThrough, false);
  const route = trace.sources.find((candidate) => candidate.existing)!;
  assert.equal(route.connected, true);
  assert.match(route.notes.join(' '), /timeout, crash, malformed, or refusal/);
  assert.match(route.notes.join(' '), /connected mapping alone does not guarantee a value/);
});
