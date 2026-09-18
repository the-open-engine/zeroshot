import test from 'node:test';
import assert from 'node:assert/strict';
import { findNode, type Document, type GraphNode } from './domain';
import { makeReadOnlyVerifier, makeWritingAgent } from './activity-access';

const worker = (name: string): GraphNode => ({
  name,
  kind: 'step',
  worker: 'agent.shared@1',
  instructions: 'Return a structured plan.',
  input: { kind: 'record', fields: { task: { type: { kind: 'string' }, required: true } } },
  output: { kind: 'record', fields: { plan: { type: { kind: 'string' }, required: true } } },
  inputBindings: [{ target: ['task'], value: { source: 'state', path: ['task'] } }],
  writeBindings: [{ target: ['plan'], value: { node: name, channel: 'out', path: ['plan'] } }],
  attempts: 2,
  timeoutMs: 50000,
  metadata: { retained: true },
});
function document(nodes = [worker('planner')]): Document {
  return {
    name: 'access-test',
    graph: {
      profile: 'openengine.graph.full/v1',
      initialInput: { kind: 'null' },
      policy: {},
      root: { kind: 'seq', name: 'run', state: { kind: 'null' }, children: nodes },
    },
    runtime: {
      harness: 'codex',
      provider: 'openai',
      size: 'small',
      nodes: Object.fromEntries(
        nodes.map((node) => [node.name, { kind: 'agent', model: 'opaque-model', effort: 'high' }])
      ),
    },
  };
}

test('explicit read-only conversion preserves authored output, bindings, identity and runtime', () => {
  const source = document(),
    before = structuredClone(source);
  const next = makeReadOnlyVerifier(source, 'planner');
  assert.deepEqual(findNode(next.graph.root, 'planner'), {
    ...worker('planner'),
    kind: 'verifier',
    signals: {},
    diagnostic: { kind: 'null' },
  });
  assert.deepEqual(next.runtime, before.runtime);
  assert.deepEqual(source, before);
});

test('a changed shared contract gets a fresh worker reference without rewriting its peers', () => {
  const source = document([worker('planner'), worker('peer')]);
  const next = makeReadOnlyVerifier(source, 'planner');
  assert.equal(findNode(next.graph.root, 'planner')!.worker, 'agent.planner@1');
  assert.deepEqual(findNode(next.graph.root, 'peer'), worker('peer'));
  assert.deepEqual(next.runtime, source.runtime);
});

test('existing Verifier signals, diagnostic and runtime are unchanged on repeat selection', () => {
  const source = makeReadOnlyVerifier(document(), 'planner');
  const node = findNode(source.graph.root, 'planner')!;
  node.signals = { verdict: ['accepted', 'rejected'] };
  node.diagnostic = { kind: 'record', fields: {} };
  assert.equal(makeReadOnlyVerifier(source, 'planner'), source);
});

test('delivery and unsupported bindings cannot be relabelled as read-only activities', () => {
  for (const kind of ['git_delivery', 'future_worker']) {
    const source = document();
    source.runtime.nodes.planner = { kind };
    const before = structuredClone(source);
    assert.throws(() => makeReadOnlyVerifier(source, 'planner'), /editable contract/);
    assert.deepEqual(source, before);
  }
});

function writableCandidate() {
  const source = makeReadOnlyVerifier(document(), 'planner');
  findNode(source.graph.root, 'planner')!.attempts = 1;
  return source;
}

test('explicit writing conversion preserves identity, runtime, dataflow and instructions', () => {
  const source = writableCandidate(),
    before = structuredClone(source);
  const next = makeWritingAgent(source, 'planner');
  const expected = { ...before.graph.root.children[0], kind: 'step' };
  delete expected.signals;
  delete expected.diagnostic;
  assert.deepEqual(findNode(next.graph.root, 'planner'), expected);
  assert.deepEqual(next.runtime, before.runtime);
  assert.deepEqual(source, before);
  assert.equal(makeWritingAgent(next, 'planner'), next);
});

test('writing conversion isolates shared contracts and preserves empty record diagnostic peers', () => {
  const source = writableCandidate();
  const planner = findNode(source.graph.root, 'planner')!;
  planner.diagnostic = { kind: 'record', fields: {} };
  source.graph.root.children.push({ ...structuredClone(planner), name: 'peer' });
  source.runtime.nodes.peer = structuredClone(source.runtime.nodes.planner);
  const before = structuredClone(source);
  const next = makeWritingAgent(source, 'planner');
  assert.equal(findNode(next.graph.root, 'planner')!.worker, 'agent.planner@1');
  assert.deepEqual(findNode(next.graph.root, 'peer'), before.graph.root.children[1]);
  assert.deepEqual(next.runtime, before.runtime);
});

test('writing conversion rejects declared outcomes, diagnostics, unsupported retries and fixed workers atomically', () => {
  for (const change of [
    (node: GraphNode) => {
      node.signals = { verdict: ['accepted', 'rejected'] };
    },
    (node: GraphNode) => {
      node.diagnostic = {
        kind: 'record',
        fields: { feedback: { type: { kind: 'string' }, required: true } },
      };
    },
    (node: GraphNode) => {
      node.diagnostic = { kind: 'future', preserve: true };
    },
    (node: GraphNode) => {
      node.attempts = 2;
    },
    (node: GraphNode) => {
      node.worker = 'builtin.git-delivery.merge@2';
    },
  ]) {
    const source = writableCandidate();
    change(findNode(source.graph.root, 'planner')!);
    const before = structuredClone(source);
    assert.throws(() => makeWritingAgent(source, 'planner'), /Remove|Attempts|editable contract/);
    assert.deepEqual(source, before);
  }
  for (const kind of ['git_delivery', 'future_worker']) {
    const source = writableCandidate();
    source.runtime.nodes.planner = { kind };
    assert.throws(() => makeWritingAgent(source, 'planner'), /editable contract/);
  }
});

test('dangling and live verifier channel references reject conversion even after fields are removed', () => {
  for (const reference of [
    {
      kind: 'in',
      value: { source: 'signal', name: 'planner', field: 'verdict' },
      labels: ['accepted'],
    },
    {
      kind: 'k_of_n',
      count: 1,
      values: [{ source: 'signal', name: 'planner', field: 'verdict' }],
      labels: ['accepted'],
    },
    { target: ['feedback'], value: { node: 'planner', channel: 'diagnostic', path: ['feedback'] } },
    { target: ['verdict'], value: { node: 'planner', channel: 'signal', path: ['verdict'] } },
  ]) {
    const source = writableCandidate();
    source.graph.root.metadata = { reference };
    const before = structuredClone(source);
    assert.throws(() => makeWritingAgent(source, 'planner'), /Disconnect/);
    assert.deepEqual(source, before);
  }
  const source = writableCandidate();
  source.graph.root.metadata = {
    guard: {
      kind: 'in',
      value: { source: 'error', name: 'planner', field: null },
      labels: ['crash'],
    },
  };
  assert.equal(findNode(makeWritingAgent(source, 'planner').graph.root, 'planner')!.kind, 'step');
});
