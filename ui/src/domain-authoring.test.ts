import test from 'node:test';
import assert from 'node:assert/strict';
import {
  addNode,
  allNodes,
  assertDocument,
  blankDocument,
  findNode,
  removeNode,
  replaceNode,
  reorder,
  uniqueAgentReference,
  type Document,
  type GraphNode,
  type Template,
} from './domain';

const state = () => ({
  kind: 'record',
  fields: {
    task: { type: { kind: 'string' }, required: true },
    feedback: { type: { kind: 'string' }, required: true },
  },
});
const agent = (name: string): GraphNode => ({
  kind: 'step',
  name,
  worker: `agent.${name}@1`,
  instructions: 'Preserve this authored instruction.',
  input: { kind: 'null' },
  output: { kind: 'null' },
  inputBindings: [],
  writeBindings: [],
  attempts: 1,
});
const template = (): Template => ({
  name: 'fixture',
  graph: {
    profile: 'openengine.graph.full/v1',
    initialInput: state(),
    policy: { policy: 'policy.native-v2@1', default: 'deny' },
    root: {
      kind: 'seq',
      name: 'run',
      state: state(),
      children: [agent('template_worker')],
      promotedStatePaths: [],
    },
  },
});
function blank(): Document {
  return blankDocument(template());
}

test('blank authoring starts with empty input and state records and no template runtime bindings', () => {
  const source = template();
  const before = structuredClone(source);
  const document = blankDocument(source);
  assertDocument(document);
  assert.deepEqual(
    allNodes(document.graph.root).map((node) => node.name),
    ['run']
  );
  assert.deepEqual(document.graph.initialInput, { kind: 'record', fields: {} });
  assert.deepEqual(document.graph.root.state, { kind: 'record', fields: {} });
  assert.deepEqual(document.runtime.nodes, {});
  assert.equal(document.runtime.harness, '');
  assert.equal(document.runtime.provider, '');
  assert.deepEqual(document.graph.policy, source.graph.policy);
  assert.deepEqual(source, before);
});

test('empty structured drafts can be authored without inventing terminal placeholders', () => {
  for (const kind of ['seq', 'par', 'choice']) {
    const original = blank();
    const created = addNode(original, 'run', kind);
    assertDocument(created.document);
    const group = findNode(created.document.graph.root, created.name)!;
    assert.deepEqual(kind === 'seq' ? group.children : group.branches, []);
    assert.equal(allNodes(created.document.graph.root).length, 2);
    assert.deepEqual(created.document.runtime.nodes, {});
    assert.deepEqual(original.graph.root.children, []);
  }
});

test('renaming and adding Agents or Verifiers never shares their worker contract identity', () => {
  for (const kind of ['step', 'verifier']) {
    let doc = blank();
    const added = addNode(doc, 'run', kind);
    const original = findNode(added.document.graph.root, added.name)!;
    doc = replaceNode(added.document, added.name, {
      ...original,
      name: 'renamed_worker',
      input: state(),
    });
    const second = addNode(doc, 'run', kind);
    const created = findNode(second.document.graph.root, second.name)!;
    assert.equal(second.name, added.name);
    assert.equal(findNode(second.document.graph.root, 'renamed_worker')!.worker, original.worker);
    assert.notEqual(created.worker, original.worker);
    assert.equal(created.worker, `agent.${added.name}.2@1`);
    assert.deepEqual(created.input, { kind: 'null' });
    assert.deepEqual(findNode(second.document.graph.root, 'renamed_worker')!.input, state());
    assert.equal(findNode(doc.graph.root, second.name), undefined);
  }
});

test('Agent reference allocation reserves authored references across nested groups and suffixes', () => {
  const root: GraphNode = {
    kind: 'seq',
    name: 'run',
    children: [
      { ...agent('peer'), worker: 'agent.agent@1' },
      { kind: 'seq', name: 'nested', children: [{ ...agent('inner'), worker: 'agent.agent.2@1' }] },
    ],
  };
  assert.equal(uniqueAgentReference(root, 'agent'), 'agent.agent.3@1');
  assert.equal(uniqueAgentReference(root, 'new'), 'agent.new@1');
});

test('choice additions preserve guarded order and keep otherwise independent', () => {
  const worker = addNode(blank(), 'run', 'step');
  const choice = addNode(worker.document, 'run', 'choice', worker.name);
  const first = addNode(choice.document, choice.name, 'fail');
  const second = addNode(first.document, choice.name, 'succeed');
  const fallback = addNode(second.document, choice.name, 'seq', undefined, true);
  assertDocument(fallback.document);
  const group = findNode(fallback.document.graph.root, choice.name)!;
  assert.deepEqual(
    group.branches.map((branch: any) => branch.node.name),
    [first.name, second.name]
  );
  assert.equal(group.branches[0].when.value.name, worker.name);
  assert.equal(group.otherwise.name, fallback.name);
  assert.equal(fallback.parent, choice.name);

  const snapshot = structuredClone(fallback.document);
  assert.throws(
    () => addNode(fallback.document, choice.name, 'step', undefined, true),
    /Remove the existing otherwise/
  );
  assert.deepEqual(fallback.document, snapshot);

  const swapped = reorder(fallback.document, choice.name, second.name, -1);
  const swappedGroup = findNode(swapped.graph.root, choice.name)!;
  assert.deepEqual(swappedGroup.branches[0], group.branches[1]);
  assert.deepEqual(swappedGroup.branches[1], group.branches[0]);
  assert.deepEqual(swappedGroup.otherwise, group.otherwise);
  assert.deepEqual(reorder(swapped, choice.name, fallback.name, -1), swapped);
});

test('removing an otherwise subtree prunes only its runtime and preserves guarded branches', () => {
  const choice = addNode(blank(), 'run', 'choice');
  const guarded = addNode(choice.document, choice.name, 'verifier');
  const fallback = addNode(guarded.document, choice.name, 'seq', undefined, true);
  const nested = addNode(fallback.document, fallback.name, 'step');
  const before = structuredClone(nested.document);
  const removed = removeNode(nested.document, choice.name, fallback.name);
  assertDocument(removed);
  assert.equal(findNode(removed.graph.root, choice.name)!.otherwise, null);
  assert.equal(Object.hasOwn(removed.runtime.nodes, nested.name), false);
  assert.deepEqual(removed.runtime.nodes[guarded.name], before.runtime.nodes[guarded.name]);
  assert.deepEqual(
    findNode(removed.graph.root, choice.name)!.branches,
    findNode(before.graph.root, choice.name)!.branches
  );
  assert.deepEqual(nested.document, before);
});

for (const kind of ['loop', 'map']) {
  test(`${kind} starts with one empty sequence body and repeated Add edits that same body`, () => {
    const original = blank();
    original.graph.root.state = state();
    const container = addNode(original, 'run', kind);
    const body = findNode(container.document.graph.root, container.name)!.body;
    assert.equal(body.kind, 'seq');
    assert.deepEqual(body.children, []);
    const first = addNode(container.document, container.name, 'verifier');
    const second = addNode(first.document, container.name, 'choice', first.name);
    assertDocument(second.document);
    assert.equal(first.parent, body.name);
    assert.equal(second.parent, body.name);
    const edited = findNode(second.document.graph.root, container.name)!.body;
    assert.equal(edited.name, body.name);
    assert.deepEqual(
      edited.children.map((node: GraphNode) => node.name),
      [first.name, second.name]
    );
    assert.deepEqual(edited.state, state());
    assert.deepEqual(body.children, []);
    assert.deepEqual(Object.keys(second.document.runtime.nodes), [first.name]);
  });

  test(`${kind} Add wraps an existing leaf once without changing identity or parent metadata`, () => {
    const original = blank();
    const existing = agent('authored_worker');
    existing.writeBindings = [
      { value: { node: existing.name, channel: 'out', path: ['value'] }, target: ['a.b'] },
      { value: { node: existing.name, channel: 'out', path: ['value'] }, target: ['a', 'b'] },
    ];
    const container: GraphNode = {
      kind,
      name: 'container',
      state: state(),
      body: existing,
      promotedStatePaths: [['feedback']],
      ...(kind === 'loop'
        ? {
            maxIterations: 10,
            until: {
              kind: 'in',
              value: { name: 'review', source: 'signal', field: 'verdict' },
              labels: ['accepted'],
            },
          }
        : { maxItems: 100, over: { source: 'state', path: ['a.b'] } }),
    };
    original.graph.root.children = [container, agent('peer')];
    original.runtime.nodes = {
      authored_worker: {
        kind: 'agent',
        model: 'custom-model',
        sessionScope: 'node_instance',
        connections: { account: ['API_KEY'] },
      },
      peer: { kind: 'agent', model: 'peer-model' },
    };
    const snapshot = structuredClone(original);
    const first = addNode(original, 'container', 'verifier', existing.name);
    const second = addNode(first.document, 'container', 'succeed', first.name);
    assertDocument(second.document);
    const result = findNode(second.document.graph.root, 'container')!;
    const { body: ignoredOriginalBody, ...originalMetadata } = container;
    const { body: resultBody, ...resultMetadata } = result;
    assert.deepEqual(resultMetadata, originalMetadata);
    assert.deepEqual(resultBody.children[0], existing);
    assert.deepEqual(
      resultBody.children.map((node: GraphNode) => node.name),
      [existing.name, first.name, second.name]
    );
    assert.deepEqual(resultBody.promotedStatePaths, [['a.b'], ['a', 'b']]);
    assert.equal(first.parent, second.parent);
    assert.equal(resultBody.name, first.parent);
    assert.deepEqual(
      second.document.runtime.nodes.authored_worker,
      original.runtime.nodes.authored_worker
    );
    assert.deepEqual(second.document.runtime.nodes.peer, original.runtime.nodes.peer);
    assert.deepEqual(second.document.runtime.nodes[first.name], { kind: 'agent', model: '' });
    assert.deepEqual(original, snapshot);
    assert.equal(ignoredOriginalBody, existing);
  });
}

test('removing the final guarded node yields a recoverable empty draft without reusing its referenced identity', () => {
  const choice = addNode(blank(), 'run', 'choice');
  const verifier = addNode(choice.document, choice.name, 'verifier');
  verifier.document.graph.root.children.push({
    kind: 'choice',
    name: 'observer',
    state: { kind: 'null' },
    promotedStatePaths: [],
    branches: [
      {
        when: {
          kind: 'in',
          value: { name: verifier.name, source: 'signal', field: 'verdict' },
          labels: ['accepted'],
        },
        node: { kind: 'fail', name: 'observed_failure', reason: 'observed_failure' },
      },
    ],
    otherwise: null,
  });
  const removed = removeNode(verifier.document, choice.name, verifier.name);
  assertDocument(removed);
  assert.deepEqual(findNode(removed.graph.root, choice.name)!.branches, []);
  assert.equal(Object.hasOwn(removed.runtime.nodes, verifier.name), false);
  const replacement = addNode(removed, choice.name, 'verifier');
  assert.notEqual(replacement.name, verifier.name);
  assert.equal(
    findNode(replacement.document.graph.root, 'observer')!.branches[0].when.value.name,
    verifier.name
  );
});
