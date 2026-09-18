import test from 'node:test';
import assert from 'node:assert/strict';
import { findNode, type Document, type GraphNode } from './domain';
import { applyWorker, workerOption, type WorkerOption } from './workers';

const agent: WorkerOption = {
  id: 'agent',
  label: 'Agent',
  runtimeKind: 'agent',
  workerRefs: [],
  runtimeBinding: { kind: 'agent', model: '' },
};
const deliveryNode: GraphNode = {
  kind: 'verifier',
  name: 'deliver',
  worker: 'builtin.git-delivery.merge@2',
  attempts: 1,
  input: { kind: 'record', fields: { title: { type: { kind: 'string' }, required: true } } },
  output: {
    kind: 'record',
    fields: { mergeRevision: { type: { kind: 'string' }, required: true } },
  },
  diagnostic: { kind: 'record', fields: { message: { type: { kind: 'string' }, required: true } } },
  signals: { delivery: ['merged', 'conflict', 'ci_failed', 'repair_required'] },
  inputBindings: [{ target: ['title'], value: { source: 'state', path: ['title'] } }],
  writeBindings: [
    {
      target: ['mergeRevision'],
      value: { node: 'deliver', channel: 'out', path: ['mergeRevision'] },
    },
  ],
};
const merge: WorkerOption = {
  id: 'git_delivery_merge',
  label: 'Git delivery · merge',
  runtimeKind: 'git_delivery',
  workerRefs: ['builtin.git-delivery.merge@2', 'builtin.git-delivery.merge@1'],
  node: deliveryNode,
  runtimeBinding: { kind: 'git_delivery', connections: { github: ['GH_TOKEN'] } },
};
const pr: WorkerOption = {
  ...merge,
  id: 'git_delivery_pr',
  label: 'Git delivery · pull request',
  workerRefs: ['builtin.git-delivery.pr@1'],
  node: { ...deliveryNode, worker: 'builtin.git-delivery.pr@1' },
};
const workers = [agent, pr, merge];
function document(): Document {
  return {
    name: 'custom',
    graph: {
      profile: 'full',
      initialInput: { kind: 'null' },
      policy: {},
      root: {
        kind: 'seq',
        name: 'run',
        state: { kind: 'null' },
        children: [
          {
            kind: 'step',
            name: 'custom_worker',
            worker: 'caller.authored@1',
            instructions: 'Keep the prompt.',
            input: { kind: 'null' },
            output: { kind: 'null' },
            attempts: 1,
            inputBindings: [],
            writeBindings: [],
          },
          { kind: 'succeed', name: 'done', output: { kind: 'null' }, bindings: [] },
        ],
        promotedStatePaths: [],
      },
    },
    runtime: {
      harness: 'codex',
      provider: 'openai',
      size: 'small',
      nodes: {
        custom_worker: {
          kind: 'agent',
          model: 'custom-model',
          sessionScope: 'node',
          connections: { tools: ['TOKEN'] },
        },
      },
    },
  };
}

test('ordinary Agent selection preserves caller-authored contract identity and runtime', () => {
  const doc = document();
  const node = findNode(doc.graph.root, 'custom_worker')!;
  assert.equal(workerOption(node, doc.runtime.nodes.custom_worker, workers), agent);
  assert.equal(applyWorker(doc, node.name, agent, workers), doc);
});

test('delivery replacement is atomic, uses the server contract and requires mappings to be authored', () => {
  const doc = document(),
    before = structuredClone(doc),
    catalogBefore = structuredClone(workers);
  const next = applyWorker(doc, 'custom_worker', merge, workers);
  const node = findNode(next.graph.root, 'custom_worker')!;
  assert.equal(node.kind, 'verifier');
  assert.equal(node.worker, deliveryNode.worker);
  assert.equal(node.attempts, 1);
  assert.equal(node.instructions, undefined);
  assert.deepEqual(node.input, deliveryNode.input);
  assert.deepEqual(node.output, deliveryNode.output);
  assert.deepEqual(node.signals, deliveryNode.signals);
  assert.deepEqual(node.diagnostic, deliveryNode.diagnostic);
  assert.deepEqual(node.inputBindings, []);
  assert.deepEqual(node.writeBindings, []);
  assert.deepEqual(next.runtime.nodes.custom_worker, merge.runtimeBinding);
  assert.deepEqual(next.graph.root.children[1], doc.graph.root.children[1]);
  assert.deepEqual(doc, before);
  assert.deepEqual(workers, catalogBefore);
});

test('existing legacy merge contracts and authored connections survive selecting their current kind', () => {
  const doc = applyWorker(document(), 'custom_worker', merge, workers);
  const node = findNode(doc.graph.root, 'custom_worker')!;
  node.worker = 'builtin.git-delivery.merge@1';
  node.output = { kind: 'record', fields: { legacy: { type: { kind: 'string' } } } };
  doc.runtime.nodes.custom_worker.connections = { customGithub: ['GH_TOKEN'] };
  assert.equal(workerOption(node, doc.runtime.nodes.custom_worker, workers), merge);
  assert.equal(applyWorker(doc, node.name, merge, workers), doc);
});

test('conversion back to Agent preserves wiring and allocates an unshared contract reference', () => {
  const doc = applyWorker(document(), 'custom_worker', merge, workers);
  const node = findNode(doc.graph.root, 'custom_worker')!;
  node.inputBindings = [{ target: ['title'], value: { source: 'state', path: ['title'] } }];
  doc.graph.root.children.push({ kind: 'step', name: 'peer', worker: 'agent.custom_worker@1' });
  doc.runtime.nodes.peer = { kind: 'agent', model: 'peer' };
  const next = applyWorker(doc, 'custom_worker', agent, workers);
  const converted = findNode(next.graph.root, 'custom_worker')!;
  assert.equal(converted.worker, 'agent.custom_worker.2@1');
  assert.deepEqual(converted.inputBindings, node.inputBindings);
  assert.deepEqual(converted.output, node.output);
  assert.deepEqual(next.runtime.nodes.custom_worker, { kind: 'agent', model: '' });
  assert.deepEqual(next.runtime.nodes.peer, doc.runtime.nodes.peer);
});

test('a second delivery worker is rejected without changing either node', () => {
  const doc = applyWorker(document(), 'custom_worker', merge, workers);
  doc.graph.root.children.push({ kind: 'verifier', name: 'peer', worker: 'agent.peer@1' });
  doc.runtime.nodes.peer = { kind: 'agent', model: 'peer' };
  const before = structuredClone(doc);
  assert.throws(() => applyWorker(doc, 'peer', pr, workers), /only one Git delivery/);
  assert.deepEqual(doc, before);
});

test('explicit delivery mode changes replace the contract while retaining node identity', () => {
  const doc = applyWorker(document(), 'custom_worker', pr, workers);
  const before = structuredClone(doc);
  const next = applyWorker(doc, 'custom_worker', merge, workers);
  assert.equal(findNode(next.graph.root, 'custom_worker')!.worker, merge.node!.worker);
  assert.equal(
    workerOption(
      findNode(next.graph.root, 'custom_worker')!,
      next.runtime.nodes.custom_worker,
      workers
    ),
    merge
  );
  assert.deepEqual(doc, before);
});

test('unknown delivery identities stay unavailable until explicitly replaced', () => {
  const doc = document();
  const node = findNode(doc.graph.root, 'custom_worker')!;
  assert.equal(workerOption(node, { kind: 'git_delivery' }, workers), undefined);
  assert.throws(() => applyWorker(doc, 'done', merge, workers), /Select an Agent or Verifier/);
});
