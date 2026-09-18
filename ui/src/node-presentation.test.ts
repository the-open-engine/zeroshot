import test from 'node:test';
import assert from 'node:assert/strict';
import { type Document, type GraphNode } from './domain';
import { nodeIcons, nodePresentation } from './node-presentation';
import type { WorkerOption } from './workers';

const workers: WorkerOption[] = [
  {
    id: 'agent',
    label: 'Agent',
    runtimeKind: 'agent',
    workerRefs: [],
    runtimeBinding: { kind: 'agent', model: '' },
  },
  {
    id: 'git_delivery_merge',
    label: 'Git delivery · merge',
    runtimeKind: 'git_delivery',
    workerRefs: ['builtin.git-delivery.merge@1', 'builtin.git-delivery.merge@2'],
    runtimeBinding: { kind: 'git_delivery' },
  },
];
const deliver = (name = 'deliver'): GraphNode => ({
  kind: 'verifier',
  name,
  worker: 'builtin.git-delivery.merge@2',
});
const seq = (name: string, nodes: GraphNode[]): GraphNode => ({
  kind: 'seq',
  name,
  children: nodes,
});
function document(root: GraphNode): Document {
  return {
    name: 'presentation-only',
    graph: { profile: 'full', initialInput: { kind: 'null' }, policy: {}, root },
    runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
  };
}

test('all five group kinds have distinct icons and delivery has its own presentation', () => {
  assert.equal(
    new Set(['seq', 'par', 'choice', 'loop', 'map'].map((kind) => nodeIcons[kind])).size,
    5
  );
  const leaf = deliver();
  const doc = document(seq('root', [leaf]));
  doc.runtime.nodes.deliver = { kind: 'git_delivery' };
  const presentation = nodePresentation(doc, leaf, workers);
  assert.equal(presentation.label, 'Git delivery');
  assert.equal(presentation.detail, 'Merge');
  assert.equal(presentation.Icon, nodeIcons.git_delivery);
  assert.notEqual(presentation.Icon, nodeIcons.verifier);
});

test('unfamiliar delivery bindings still have delivery presentation', () => {
  const leaf = { ...deliver(), worker: 'caller.unknown@1' };
  const doc = document(seq('root', [leaf]));
  doc.runtime.nodes.deliver = { kind: 'git_delivery' };
  const before = structuredClone(doc);
  assert.equal(nodePresentation(doc, leaf, workers).delivery, true);
  assert.equal(nodePresentation(doc, leaf, workers).Icon, nodeIcons.git_delivery);
  assert.deepEqual(doc, before);
});
