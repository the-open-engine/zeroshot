import test from 'node:test';
import assert from 'node:assert/strict';
import {
  assertDocument,
  addNode,
  connectAfter,
  removeNode,
  replaceNode,
  reorder,
  replaceBody,
  wrapBody,
  uniqueName,
  type Document,
} from './domain';
const node = (name: string) => ({
  kind: 'step',
  name,
  instructions: 'Mention worker literally; do not rename this prose.',
  input: { kind: 'null' },
  output: { kind: 'null' },
  inputBindings: [],
  writeBindings: [],
  attempts: 1,
  worker: 'agent@1',
});
function fixture(): Document {
  return {
    name: 'test',
    graph: {
      profile: 'full',
      initialInput: { kind: 'null' },
      policy: {},
      root: {
        kind: 'seq',
        name: 'root',
        state: { kind: 'null' },
        children: [
          node('worker'),
          {
            ...node('review'),
            writeBindings: [{ value: { node: 'worker', channel: 'out', path: [] }, target: [] }],
          },
          {
            kind: 'choice',
            name: 'route',
            branches: [
              {
                when: { kind: 'in', value: { name: 'worker', source: 'error' }, labels: ['crash'] },
                node: { kind: 'fail', name: 'failed', reason: 'failed' },
              },
            ],
            otherwise: { kind: 'succeed', name: 'done', output: { kind: 'null' }, bindings: [] },
          },
        ],
        promotedStatePaths: [],
      },
    },
    runtime: {
      harness: 'codex',
      provider: 'openai',
      size: 'small',
      nodes: {
        worker: { kind: 'agent', model: 'opaque/model' },
        review: { kind: 'agent', model: 'another' },
      },
    },
  };
}
test('rename carries runtime and structured references, but never rewrites prompt text', () => {
  const d = fixture(),
    renamed = replaceNode(d, 'worker', { ...node('implement') });
  assert.equal(renamed.runtime.nodes.implement.model, 'opaque/model');
  assert.equal(renamed.runtime.nodes.worker, undefined);
  assert.equal(renamed.graph.root.children[1].writeBindings[0].value.node, 'implement');
  assert.equal(renamed.graph.root.children[2].branches[0].when.value.name, 'implement');
  assert.match(renamed.graph.root.children[0].instructions, /worker literally/);
  assert.equal(d.graph.root.children[0].name, 'worker');
});
test('rename collision fails without mutating original document', () => {
  const d = fixture();
  assert.throws(() => replaceNode(d, 'worker', node('review')), /already exists/);
  assert.equal(d.runtime.nodes.worker.model, 'opaque/model');
});
test('connecting in either direction gives a stable semantic sequence', () => {
  const d = fixture();
  assert.deepEqual(
    connectAfter(d, 'root', 'route', 'worker').graph.root.children.map((n: any) => n.name),
    ['review', 'route', 'worker']
  );
  assert.deepEqual(
    connectAfter(d, 'root', 'worker', 'route').graph.root.children.map((n: any) => n.name),
    ['worker', 'route', 'review']
  );
  assert.throws(() => connectAfter(d, 'root', 'worker', 'worker'), /itself/);
});
test('removing a subtree prunes bindings without destroying references awaiting repair', () => {
  const d = fixture();
  const removed = removeNode(d, 'root', 'worker');
  assert.equal(removed.runtime.nodes.worker, undefined);
  assert.equal(removed.graph.root.children[0].writeBindings[0].value.node, 'worker');
  assert.equal(d.graph.root.children.length, 3);
});
test('otherwise branch does not move into guarded branches', () => {
  const d = fixture();
  assert.deepEqual(reorder(d, 'route', 'done', -1), d);
});
test('root deletion fails closed and an empty group remains an editable draft', () => {
  const d = fixture();
  assert.throws(() => removeNode(d, 'root', 'root'), /root/);
  const empty = removeNode(d, 'route', 'failed');
  assert.equal(empty.graph.root.children[2].branches.length, 0);
  assertDocument(empty);
});
test('malformed imports fail before recursive rendering', () => {
  for (const value of [null, [], {}, { graph: { root: null }, runtime: { nodes: {} } }])
    assert.throws(() => assertDocument(value));
  const d = fixture();
  d.graph.root.children.push(node('worker'));
  assert.throws(() => assertDocument(d), /Duplicate/);
  const bad = fixture();
  bad.graph.root = { kind: 'loop', name: 'loop', body: null };
  assert.throws(() => assertDocument(bad));
});
test('advanced graph and runtime fields survive graph operations', () => {
  const d = fixture();
  d.runtime.nodes.worker.connections = { account: ['API_KEY'] };
  d.runtime.nodes.worker.sessionScope = 'node_instance';
  const changed = reorder(d, 'root', 'review', -1);
  assert.deepEqual(changed.runtime, d.runtime);
  assertDocument(changed);
});

test('malformed render-facing fields stay out of the canvas', () => {
  for (const bad of [null, { x: 1 }, ['model']]) {
    const d = fixture();
    d.runtime.nodes.worker.model = bad as any;
    assert.throws(() => assertDocument(d), /must be text/);
  }
  const d = fixture();
  d.graph.root = { kind: 'fail', name: 'bad', reason: { x: 1 } };
  assert.throws(() => assertDocument(d), /must be text/);
  d.graph.root = { kind: ['step'] as any, name: 'bad' };
  assert.throws(() => assertDocument(d), /supported kind/);
  d.graph.root = { kind: '__proto__', name: 'bad' };
  assert.throws(() => assertDocument(d), /supported kind/);
});
test('reserved JavaScript property names remain ordinary node keys', () => {
  for (const name of ['__proto__', 'constructor', 'toString']) {
    const d = replaceNode(fixture(), 'worker', node(name));
    assert.equal(Object.hasOwn(d.runtime.nodes, name), true);
    assert.equal(d.runtime.nodes[name].model, 'opaque/model');
    assert.equal(JSON.parse(JSON.stringify(d)).runtime.nodes[name].model, 'opaque/model');
    assertDocument(d);
  }
});
test('new group children cannot collide with an existing graph node', () => {
  for (const kind of ['seq', 'par', 'loop']) {
    const d = fixture();
    d.graph.root.children.push({
      kind: 'succeed',
      name: kind + '_done',
      output: { kind: 'null' },
      bindings: [],
    });
    const next = addNode(d, 'root', kind).document;
    assertDocument(next);
  }
});

for (const kind of ['loop', 'map']) {
  test(`${kind} body replacement preserves parent metadata and external references`, () => {
    const d = fixture();
    const container = {
      kind,
      name: 'container',
      body: d.graph.root.children[0],
      state: { kind: 'null' },
      maxIterations: 3,
      maxItems: 4,
      over: { source: 'state', path: ['a.b'] },
      until: { kind: 'opaque' },
      promotedStatePaths: [['x']],
    };
    d.graph.root.children[0] = container;
    const replaced = replaceBody(d, 'container', 'seq');
    assertDocument(replaced);
    const c = replaced.graph.root.children[0];
    assert.equal(c.body.kind, 'seq');
    assert.deepEqual(c.over, container.over);
    assert.deepEqual(c.until, container.until);
    assert.deepEqual(c.promotedStatePaths, container.promotedStatePaths);
    assert.equal(replaced.runtime.nodes.worker, undefined);
    assert.deepEqual(replaced.runtime.nodes.review, d.runtime.nodes.review);
    assert.equal(replaced.graph.root.children[1].writeBindings[0].value.node, 'worker');
    assert.equal(d.graph.root.children[0].body.name, 'worker');
    const agent = replaceBody(replaced, 'container', 'step');
    const name = agent.graph.root.children[0].body.name;
    assert.deepEqual(agent.runtime.nodes[name], { kind: 'agent', model: '' });
  });
}
test('wrapping preserves runtime identity, literal path segments, and exposed writes', () => {
  const d = fixture(),
    worker = d.graph.root.children[0];
  worker.writeBindings = [{ target: ['a.b'] }, { target: ['a', 'b'] }, { target: ['a.b'] }];
  d.graph.root.children[0] = {
    kind: 'loop',
    name: 'loop',
    body: worker,
    state: { kind: 'null' },
    promotedStatePaths: [],
  };
  const wrapped = wrapBody(d, 'loop');
  const seq = wrapped.document.graph.root.children[0].body;
  assert.equal(seq.name, wrapped.name);
  assert.deepEqual(seq.children[0], worker);
  assert.deepEqual(seq.promotedStatePaths, [['a.b'], ['a', 'b']]);
  assert.deepEqual(wrapped.document.runtime, d.runtime);
});
test('generated identities cannot rebind dangling guards or output references', () => {
  const d = fixture();
  d.graph.root.children[0] = {
    kind: 'loop',
    name: 'loop',
    state: { kind: 'null' },
    body: { ...node('verifier'), kind: 'verifier' },
    until: {
      kind: 'in',
      value: { source: 'signal', name: 'verifier', field: 'verdict' },
      labels: ['accepted'],
    },
    promotedStatePaths: [],
    maxIterations: 3,
  };
  const first = replaceBody(d, 'loop', 'verifier');
  const second = replaceBody(first, 'loop', 'verifier');
  assert.equal(first.graph.root.children[0].body.name, 'verifier_2');
  assert.equal(second.graph.root.children[0].body.name, 'verifier_3');
  assert.equal(second.graph.root.children[0].until.value.name, 'verifier');
  assert.equal(uniqueName(second.graph.root, 'worker'), 'worker_2');
  assert.equal(uniqueName(second.graph.root, 'mentioned_in_prose'), 'mentioned_in_prose');
});

test('portable profile JSON rejects environment references and definitions', () => {
  const doc = fixture();
  assertDocument(doc);
  for (const environment of [
    { id: 'environment-1' },
    { setup: 'apt-get install make' },
    { startup: 'npm ci' },
    { id: 'environment-1', startup: 'hidden override' },
    {},
    [],
    null,
    { id: '' },
    { id: ' ' },
    { id: 'x'.repeat(257) },
  ]) {
    assert.throws(
      () =>
        assertDocument(
          JSON.parse(
            JSON.stringify({
              ...doc,
              runtime: { ...doc.runtime, environment },
            })
          )
        ),
      /Environments belong to run submission/
    );
  }
});
