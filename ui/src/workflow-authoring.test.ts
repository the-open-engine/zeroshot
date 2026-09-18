import test from 'node:test';
import assert from 'node:assert/strict';
import { allNodes, children, findNode, pathTo, type Document, type GraphNode } from './domain';
import {
  addNext,
  addOutcome,
  appendActivity,
  automaticProtectionTarget,
  parallelSiblingCandidates,
  wrapActivity,
  wrapParallelActivities,
} from './workflow-authoring';

const schema = {
  kind: 'record',
  fields: { feedback: { required: true, type: { kind: 'string' } } },
};
const leaf = (name = 'activity'): GraphNode => ({
  kind: 'step',
  name,
  worker: `authored.${name}@1`,
  instructions: 'Preserve the authored prompt.',
  input: { kind: 'null' },
  output: { kind: 'record', fields: { message: { required: true, type: { kind: 'string' } } } },
  inputBindings: [],
  writeBindings: [
    { target: ['feedback'], value: { node: name, channel: 'out', path: ['message'] } },
  ],
  attempts: 1,
});
const sequence = (name: string, nodes: GraphNode[], state = schema): GraphNode => ({
  kind: 'seq',
  name,
  state,
  children: nodes,
  promotedStatePaths: [],
});
const terminal = (kind = 'succeed'): GraphNode =>
  kind === 'succeed'
    ? { kind, name: 'done', output: { kind: 'null' }, bindings: [] }
    : { kind, name: 'failed', reason: 'failed' };
function doc(root = sequence('root', [leaf(), terminal()])): Document {
  return {
    name: 'authored',
    graph: {
      profile: 'openengine.graph.full/v1',
      policy: { policy: 'policy.native-v2@1', default: 'deny' },
      initialInput: schema,
      root,
    },
    runtime: {
      harness: 'codex',
      provider: 'openai',
      size: 'small',
      nodes: Object.fromEntries(
        allNodes(root)
          .filter((node) => ['step', 'verifier'].includes(node.kind))
          .map((node) => [
            node.name,
            {
              kind: 'agent',
              model: 'caller-model',
              sessionScope: 'node',
              connections: { tools: ['TOKEN'] },
            },
          ])
      ),
    },
  };
}
const guard = {
  kind: 'in',
  value: { name: 'activity', source: 'error', field: null },
  labels: ['crash'],
};

test('Add next inserts directly after the chosen sequence child and preserves all existing nodes', () => {
  const source = doc(),
    before = structuredClone(source);
  const next = addNext(source, 'activity', 'verifier');
  assert.equal(next.scope, 'root');
  assert.deepEqual(
    children(next.document.graph.root).map((node) => node.name),
    ['activity', next.name, 'done']
  );
  assert.deepEqual(
    findNode(next.document.graph.root, 'activity'),
    findNode(source.graph.root, 'activity')
  );
  assert.deepEqual(next.document.runtime.nodes.activity, source.runtime.nodes.activity);
  assert.equal(next.document.runtime.nodes[next.name].model, '');
  assert.deepEqual(source, before);
});

test('Add next in a guarded outcome wraps only that branch without changing guard or otherwise', () => {
  const choice: GraphNode = {
    kind: 'choice',
    name: 'decision',
    state: schema,
    branches: [{ when: guard, node: leaf() }],
    otherwise: leaf('fallback'),
    promotedStatePaths: [['feedback']],
  };
  const source = doc(sequence('root', [choice]));
  const next = addNext(source, 'activity', 'step');
  const changed = findNode(next.document.graph.root, 'decision')!;
  assert.deepEqual(changed.branches[0].when, guard);
  assert.deepEqual(changed.otherwise, choice.otherwise);
  assert.equal(changed.branches[0].node.name, next.scope);
  assert.deepEqual(
    changed.branches[0].node.children.map((node: GraphNode) => node.name),
    ['activity', next.name]
  );
  assert.deepEqual(changed.branches[0].node.promotedStatePaths, [['feedback']]);
  assert.deepEqual(changed.promotedStatePaths, choice.promotedStatePaths);
  assert.equal(changed.branches[0].when.value.name, 'activity');
});

for (const kind of ['par', 'loop', 'map']) {
  test(`Add next in a ${kind} preserves its exact body/branch scope and enclosing schema`, () => {
    const state = {
      kind: 'record',
      fields: { feedback: { required: true, type: { kind: 'array', items: { kind: 'string' } } } },
    };
    const parent: GraphNode = {
      kind,
      name: 'container',
      state,
      promotedStatePaths: [['feedback']],
      ...(kind === 'par'
        ? { branches: [leaf(), leaf('peer')], join: { kind: 'all' } }
        : {
            body: leaf(),
            ...(kind === 'loop'
              ? { maxIterations: 7, until: guard }
              : { maxItems: 8, over: { source: 'state', path: ['feedback'] } }),
          }),
    };
    const source = doc(sequence('root', [parent], state)),
      before = structuredClone(source);
    const next = addNext(source, 'activity', 'step');
    const scope = findNode(next.document.graph.root, next.scope)!;
    assert.deepEqual(scope.state, state);
    assert.deepEqual(scope.promotedStatePaths, [['feedback']]);
    assert.equal(pathTo(next.document.graph.root, next.name).at(-2)!.name, next.scope);
    assert.equal(pathTo(next.document.graph.root, next.scope).at(-2)!.name, 'container');
    const changed = findNode(next.document.graph.root, 'container')!;
    assert.deepEqual(changed.state, state);
    if (kind === 'par') assert.deepEqual(changed.branches[1], parent.branches[1]);
    if (kind === 'loop') {
      assert.equal(changed.maxIterations, 7);
      assert.deepEqual(changed.until, guard);
    }
    if (kind === 'map') {
      assert.equal(changed.maxItems, 8);
      assert.deepEqual(changed.over, parent.over);
    }
    assert.deepEqual(source, before);
  });
}

test('root activity gets a sequence with caller-owned initial schema, without renaming its references', () => {
  const source = doc(leaf());
  const next = addNext(source, 'activity', 'verifier');
  assert.equal(next.document.graph.root.name, next.scope);
  assert.deepEqual(next.document.graph.root.state, source.graph.initialInput);
  assert.deepEqual(next.document.graph.initialInput, source.graph.initialInput);
  assert.equal(next.document.graph.root.children[0].name, 'activity');
  assert.deepEqual(
    next.document.graph.root.children[0].writeBindings,
    source.graph.root.writeBindings
  );
  assert.deepEqual(next.document.runtime.nodes.activity, source.runtime.nodes.activity);
});

test('Add outcome appends one guarded branch and never replaces otherwise or prior guards', () => {
  const choice: GraphNode = {
    kind: 'choice',
    name: 'decision',
    state: schema,
    branches: [{ when: guard, node: leaf() }],
    otherwise: terminal(),
    promotedStatePaths: [],
  };
  const source = doc(choice),
    before = structuredClone(source);
  const next = addOutcome(source, 'decision', 'step');
  const changed = next.document.graph.root;
  assert.equal(next.scope, 'decision');
  assert.deepEqual(changed.branches[0], choice.branches[0]);
  assert.equal(changed.branches[1].node.name, next.name);
  assert.deepEqual(changed.otherwise, choice.otherwise);
  assert.deepEqual(source, before);
});

for (const kind of ['loop', 'par'] as const) {
  test(`wrapping an activity in ${kind} preserves identity, runtime and exact exposed paths`, () => {
    const source = doc();
    const activity = findNode(source.graph.root, 'activity')!;
    activity.writeBindings = [
      ...activity.writeBindings,
      { target: ['a.b'], value: { node: 'activity', channel: 'out', path: ['message'] } },
      { target: ['a', 'b'], value: { node: 'activity', channel: 'out', path: ['message'] } },
      activity.writeBindings[0],
    ];
    const before = structuredClone(source);
    const next = wrapActivity(source, 'activity', kind);
    const wrapper = findNode(next.document.graph.root, next.name)!;
    assert.equal(next.scope, 'root');
    assert.deepEqual(wrapper.state, schema);
    assert.deepEqual(wrapper.promotedStatePaths, [['feedback'], ['a.b'], ['a', 'b']]);
    assert.deepEqual(findNode(next.document.graph.root, 'activity'), activity);
    assert.deepEqual(next.document.runtime, source.runtime);
    assert.deepEqual(findNode(next.document.graph.root, 'done'), terminal());
    if (kind === 'loop') {
      assert.equal(wrapper.maxIterations, 3);
      assert.equal(wrapper.until, undefined);
    } else assert.deepEqual(wrapper.join, { kind: 'all' });
    assert.deepEqual(source, before);
  });
}

test('wrapping structured activities copies their promotions and preserves reserved wrapper identities', () => {
  const inner = sequence('inner', [leaf()]);
  inner.promotedStatePaths = [['feedback']];
  const observer: GraphNode = {
    kind: 'choice',
    name: 'observer',
    state: schema,
    branches: [
      {
        when: {
          kind: 'in',
          value: { name: 'inner_loop', source: 'group', field: 'terminated' },
          labels: ['exhausted'],
        },
        node: terminal(),
      },
    ],
    promotedStatePaths: [],
  };
  const source = doc(sequence('root', [inner, observer]));
  const next = wrapActivity(source, 'inner', 'loop');
  assert.equal(next.name, 'inner_loop_2');
  assert.deepEqual(findNode(next.document.graph.root, next.name)!.promotedStatePaths, [
    ['feedback'],
  ]);
  assert.deepEqual(findNode(next.document.graph.root, 'inner'), inner);
  assert.deepEqual(findNode(next.document.graph.root, 'observer'), observer);
});

test('terminal and invalid authoring actions fail without changing the document', () => {
  for (const kind of ['succeed', 'fail']) {
    const source = doc(sequence('root', [leaf(), terminal(kind)])),
      before = structuredClone(source);
    assert.throws(() => addNext(source, terminal(kind).name, 'step'), /terminal ends the run/);
    assert.throws(() => wrapActivity(source, terminal(kind).name, 'loop'), /terminal ends the run/);
    assert.deepEqual(source, before);
  }
  const source = doc(),
    before = structuredClone(source);
  assert.throws(() => addNext(source, 'missing', 'step'), /no longer exists/);
  assert.throws(() => addOutcome(source, 'activity', 'step'), /Choose a decision/);
  assert.throws(() => addNext(source, 'activity', 'unsupported'), /supported node type/);
  assert.deepEqual(source, before);
});

test('unsupported authored exposure formats are rejected without silently dropping data', () => {
  const source = doc(leaf());
  source.graph.root.writeBindings = { future: [] };
  const before = structuredClone(source);
  assert.throws(() => addNext(source, 'activity', 'step'), /unsupported writes/);
  assert.throws(() => wrapActivity(source, 'activity', 'par'), /unsupported writes/);
  assert.deepEqual(source, before);
});

test('append rejects terminal sequence paths, nested bodies and disconnected trailing drafts', () => {
  for (const root of [
    sequence('root', [sequence('nested', [leaf(), terminal('fail')])]),
    sequence('root', [terminal(), leaf()]),
    {
      kind: 'loop',
      name: 'root',
      state: schema,
      maxIterations: 3,
      body: terminal(),
      promotedStatePaths: [],
    },
    {
      kind: 'map',
      name: 'root',
      state: schema,
      maxItems: 3,
      over: { source: 'state', path: ['items'] },
      body: sequence('body', [leaf(), terminal()]),
      promotedStatePaths: [],
    },
  ]) {
    const source = doc(root),
      before = structuredClone(source);
    assert.throws(() => appendActivity(source, 'root', 'step'), /ends this execution path/);
    assert.deepEqual(source, before);
  }
});

test('append cannot bypass a terminal-only decision', () => {
  const choice: GraphNode = {
    kind: 'choice',
    name: 'decision',
    state: schema,
    branches: [{ when: guard, node: terminal() }],
    otherwise: terminal('fail'),
    promotedStatePaths: [],
  };
  assert.throws(
    () => appendActivity(doc(sequence('root', [choice])), 'root', 'step'),
    /ends this execution path/
  );
});

test('parallel terminal branches and unmet joins permit a following group-result checkpoint', () => {
  for (const join of [
    { kind: 'all' },
    { kind: 'any' },
    { kind: 'quorum', count: 2 },
    { kind: 'first', when: guard },
  ]) {
    for (const branches of [
      [terminal('fail'), leaf()],
      [terminal('fail'), terminal()],
      [sequence('failed_branch', [terminal('fail')]), sequence('finished_branch', [terminal()])],
    ]) {
      const parallel: GraphNode = {
        kind: 'par',
        name: 'parallel',
        state: schema,
        branches,
        join,
        promotedStatePaths: [],
      };
      const source = doc(sequence('root', [parallel]));
      const before = structuredClone(source);
      const next = appendActivity(source, 'root', 'choice');
      assert.deepEqual(
        children(next.document.graph.root).map((node) => node.name),
        ['parallel', next.name]
      );
      assert.equal(findNode(next.document.graph.root, next.name)!.kind, 'choice');
      assert.deepEqual(findNode(next.document.graph.root, 'parallel'), parallel);
      assert.deepEqual(next.document.runtime, source.runtime);
      assert.deepEqual(source, before);
    }
  }
});

test('parallel terminal locality survives nested scopes without allowing work after a branch terminal', () => {
  const stopped = sequence('stopped_branch', [terminal('fail')]);
  const parallel: GraphNode = {
    kind: 'par',
    name: 'parallel',
    state: schema,
    branches: [stopped, leaf()],
    join: { kind: 'all' },
    promotedStatePaths: [],
  };
  const loop: GraphNode = {
    kind: 'loop',
    name: 'repeat',
    state: schema,
    maxIterations: 3,
    body: sequence('iteration', [parallel]),
    promotedStatePaths: [],
  };
  const source = doc(sequence('root', [loop]));
  for (const owner of ['root', 'repeat', 'iteration']) {
    const next = appendActivity(source, owner, 'step');
    assert.equal(findNode(next.document.graph.root, next.name)!.kind, 'step');
    assert.deepEqual(findNode(next.document.graph.root, 'parallel'), parallel);
    assert.deepEqual(next.document.runtime.nodes.activity, source.runtime.nodes.activity);
  }
  assert.throws(
    () => appendActivity(source, stopped.name, 'step'),
    /failed ends this execution path/
  );
});

test('append permits empty sequences and nonterminal nested bodies without changing their state', () => {
  for (const root of [
    sequence('root', []),
    {
      kind: 'loop',
      name: 'root',
      state: schema,
      maxIterations: 3,
      body: leaf(),
      promotedStatePaths: [['feedback']],
    },
    {
      kind: 'map',
      name: 'root',
      state: schema,
      maxItems: 3,
      over: { source: 'state', path: ['items'] },
      body: sequence('body', [leaf()]),
      promotedStatePaths: [],
    },
  ]) {
    const source = doc(root),
      before = structuredClone(source);
    const next = appendActivity(source, 'root', 'verifier');
    assert.equal(findNode(next.document.graph.root, next.name)!.kind, 'verifier');
    assert.equal(pathTo(next.document.graph.root, next.name).at(-2)!.name, next.scope);
    assert.deepEqual(next.document.graph.root.state, source.graph.root.state);
    assert.deepEqual(source, before);
  }
});

test('appending a parallel or decision alternative does not add after its terminal sibling', () => {
  const parallel: GraphNode = {
    kind: 'par',
    name: 'parallel',
    state: schema,
    branches: [terminal()],
    join: { kind: 'all' },
    promotedStatePaths: [],
  };
  const choice: GraphNode = {
    kind: 'choice',
    name: 'decision',
    state: schema,
    branches: [{ when: guard, node: terminal() }],
    otherwise: terminal('fail'),
    promotedStatePaths: [],
  };
  for (const root of [parallel, choice]) {
    const source = doc(root);
    const next = appendActivity(source, root.name, 'step');
    assert.equal(next.scope, root.name);
    assert.deepEqual(next.document.graph.root.branches[0], root.branches[0]);
    assert.equal(findNode(next.document.graph.root, next.name)!.kind, 'step');
  }
});

test('a terminal map body does not imply terminal outer flow when the map has no items', () => {
  const map: GraphNode = {
    kind: 'map',
    name: 'map',
    state: schema,
    maxItems: 3,
    over: { source: 'state', path: ['items'] },
    body: terminal(),
    promotedStatePaths: [],
  };
  const source = doc(sequence('root', [map]));
  const next = appendActivity(source, 'root', 'step');
  assert.equal(next.scope, 'root');
  assert.deepEqual(next.document.graph.root.children[0], map);
});

test('append inserts before ordinary final completion without changing its output contract', () => {
  const source = doc();
  const next = appendActivity(source, 'root', 'step');
  assert.deepEqual(
    children(next.document.graph.root).map((node) => node.name),
    ['activity', next.name, 'done']
  );
  assert.deepEqual(findNode(next.document.graph.root, 'done'), findNode(source.graph.root, 'done'));
});

test('adding after a protected worker preserves the error checkpoint ahead of its new continuation', () => {
  const fail = terminal('fail');
  const errorGuard = { ...guard, labels: ['timeout', 'crash', 'malformed', 'refusal'] };
  const choice: GraphNode = {
    kind: 'choice',
    name: 'worker_result',
    state: schema,
    promotedStatePaths: [],
    branches: [{ when: errorGuard, node: fail }],
    otherwise: terminal(),
  };
  const source = doc(sequence('root', [leaf(), choice]));
  const before = structuredClone(source);
  const next = addNext(source, 'activity', 'step');
  const changed = findNode(next.document.graph.root, 'worker_result')!;
  assert.deepEqual(changed.branches, choice.branches);
  assert.equal(changed.otherwise.kind, 'seq');
  assert.deepEqual(
    changed.otherwise.children.map((node: GraphNode) => node.name),
    [next.name, 'done']
  );
  assert.deepEqual(source, before);
  const appended = appendActivity(source, 'root', 'step');
  assert.deepEqual(
    findNode(appended.document.graph.root, 'worker_result')!.branches,
    choice.branches
  );
});

const independent = (name: string): GraphNode => ({
  ...leaf(name),
  output: { kind: 'null' },
  writeBindings: [],
});

test('adjacent existing writers move into parallel atomically with identity, state and runtime intact', () => {
  const writers = ['venue', 'agenda', 'budget'].map(independent);
  const source = doc(sequence('root', [...writers, terminal()]));
  const before = structuredClone(source);
  const next = wrapParallelActivities(source, 'agenda', ['venue', 'budget']);
  const group = findNode(next.document.graph.root, next.name)!;
  assert.deepEqual(group.branches, writers);
  assert.deepEqual(group.state, source.graph.root.state);
  assert.deepEqual(group.join, { kind: 'all' });
  assert.deepEqual(group.promotedStatePaths, []);
  assert.deepEqual(next.document.runtime, source.runtime);
  assert.deepEqual(
    children(next.document.graph.root).map((node) => node.name),
    [next.name, 'done']
  );
  assert.deepEqual(source, before);
  assert.deepEqual(
    parallelSiblingCandidates(source, 'agenda').map((node) => node.name),
    ['venue', 'agenda', 'budget']
  );
  assert.deepEqual(wrapParallelActivities(source, 'venue'), wrapActivity(source, 'venue', 'par'));
});

test('parallel grouping preserves distinct output writes and promotes each target once', () => {
  const a = leaf('a'),
    b = leaf('b');
  b.writeBindings[0].target = ['other'];
  const source = doc(sequence('root', [a, b, terminal()]));
  const next = wrapParallelActivities(source, 'a', ['b']);
  const group = findNode(next.document.graph.root, next.name)!;
  assert.deepEqual(group.promotedStatePaths, [['feedback'], ['other']]);
  assert.deepEqual(group.branches, [a, b]);
});

test('parallel grouping rejects forward data dependencies and overlapping writes without mutations', () => {
  const a = leaf('a'),
    b = independent('b');
  b.inputBindings = [{ target: ['brief'], value: { source: 'state', path: ['feedback'] } }];
  const source = doc(sequence('root', [a, b, terminal()]));
  const before = structuredClone(source);
  assert.throws(() => wrapParallelActivities(source, 'a', ['b']), /uses a result from a/);
  assert.deepEqual(source, before);
  assert.deepEqual(
    parallelSiblingCandidates(source, 'a').map((node) => node.name),
    ['a']
  );
  b.inputBindings = [];
  b.writeBindings = [
    { target: ['feedback', 'nested'], value: { node: 'b', channel: 'out', path: [] } },
  ];
  const collision = doc(sequence('root', [a, b, terminal()]));
  assert.throws(() => wrapParallelActivities(collision, 'a', ['b']), /overlapping fields/);
});

test('parallel grouping cannot skip or cross a decision/checkpoint or change native workers', () => {
  const checkpoint: GraphNode = {
    kind: 'choice',
    name: 'check',
    state: schema,
    promotedStatePaths: [],
    branches: [{ when: guard, node: terminal('fail') }],
    otherwise: independent('continuation'),
  };
  const source = doc(
    sequence('root', [independent('a'), checkpoint, independent('b'), terminal()])
  );
  assert.throws(() => wrapParallelActivities(source, 'a', ['b']), /contiguous/);
  assert.throws(() => wrapParallelActivities(source, 'a', ['check', 'b']), /Agent-backed/);
  assert.deepEqual(
    parallelSiblingCandidates(source, 'a').map((node) => node.name),
    ['a']
  );
  const native = independent('native');
  native.worker = 'builtin.git-delivery@1';
  const withNative = doc(sequence('root', [independent('a'), native, terminal()]));
  assert.throws(() => wrapParallelActivities(withNative, 'a', ['native']), /Agent-backed/);
});

test('parallel grouping checks the full selected interval and rejects cross-activity channel references', () => {
  const a = leaf('a'),
    b = independent('b'),
    c = leaf('c');
  const source = doc(sequence('root', [a, b, c, terminal()]));
  assert.deepEqual(
    parallelSiblingCandidates(source, 'b').map((node) => node.name),
    ['a', 'b', 'c']
  );
  assert.throws(() => wrapParallelActivities(source, 'b', ['a', 'c']), /overlapping fields/);
  b.writeBindings = [
    { target: ['other'], value: { node: 'a', channel: 'out', path: ['message'] } },
  ];
  assert.throws(
    () => wrapParallelActivities(doc(sequence('root', [a, b, terminal()])), 'a', ['b']),
    /references a/
  );
});

test('an insertion after parallel protects its fresh writer inside a business review loop', () => {
  const parallel: GraphNode = {
    kind: 'par',
    name: 'drafts',
    state: schema,
    branches: [independent('email'), independent('social')],
    join: { kind: 'all' },
    promotedStatePaths: [],
  };
  const review = {
    ...parallel,
    name: 'reviews',
    branches: [independent('brand'), independent('content')],
  };
  const decision: GraphNode = {
    kind: 'choice',
    name: 'review_result',
    state: schema,
    branches: [{ when: guard, node: terminal() }],
    otherwise: independent('revise'),
    promotedStatePaths: [],
  };
  const loop: GraphNode = {
    kind: 'loop',
    name: 'campaign',
    state: schema,
    maxIterations: 3,
    body: sequence('body', [parallel, review, decision]),
    promotedStatePaths: [],
  };
  const source = doc(sequence('root', [loop, terminal('fail')]));
  const next = addNext(source, 'drafts', 'step');
  assert.deepEqual(
    findNode(next.document.graph.root, 'body')!.children.map((node: GraphNode) => node.name),
    ['drafts', next.name, 'reviews', 'review_result']
  );
  assert.equal(automaticProtectionTarget(next.document, next.name), next.name);
  assert.deepEqual(findNode(next.document.graph.root, 'review_result'), decision);
});

test('automatic protection preserves supported outer group handling and authored recovery', () => {
  const loop: GraphNode = {
    kind: 'loop',
    name: 'repeat',
    state: schema,
    maxIterations: 3,
    body: sequence('body', [independent('writer'), independent('review')]),
    promotedStatePaths: [],
  };
  assert.equal(
    automaticProtectionTarget(doc(sequence('root', [loop, terminal()])), 'writer'),
    'repeat'
  );
  const recovery: GraphNode = {
    kind: 'choice',
    name: 'recovery',
    state: schema,
    branches: [{ when: guard, node: independent('repair') }],
    otherwise: terminal(),
    promotedStatePaths: [],
  };
  assert.equal(
    automaticProtectionTarget(doc(sequence('root', [independent('writer'), recovery])), 'writer'),
    undefined
  );
  assert.equal(
    automaticProtectionTarget(doc(sequence('root', [independent('writer')])), 'writer'),
    undefined
  );
});

test('unsupported parallel or map regions do not get local run-terminal fallback protection', () => {
  const business: GraphNode = {
    kind: 'choice',
    name: 'route',
    state: schema,
    branches: [{ when: guard, node: independent('other') }],
    otherwise: independent('fallback'),
    promotedStatePaths: [],
  };
  const body = sequence('body', [independent('writer'), independent('review'), business]);
  for (const group of [
    {
      kind: 'par',
      name: 'group',
      state: schema,
      branches: [body],
      join: { kind: 'all' },
      promotedStatePaths: [],
    },
    {
      kind: 'map',
      name: 'group',
      state: schema,
      body,
      over: { source: 'state', path: ['items'] },
      maxItems: 3,
      promotedStatePaths: [],
    },
  ])
    assert.equal(
      automaticProtectionTarget(doc(sequence('root', [group, terminal()])), 'writer'),
      undefined
    );
});

const workerError = (name: string) => ({
  kind: 'in',
  value: { name, source: 'error', field: null },
  labels: ['timeout', 'crash', 'malformed', 'refusal'],
});
const verdict = (name: string) => ({
  kind: 'in',
  value: { name, source: 'signal', field: 'verdict' },
  labels: ['accepted'],
});
const decision = (name: string, when: unknown, otherwise = terminal()): GraphNode => ({
  kind: 'choice',
  name,
  state: schema,
  branches: [{ when, node: terminal('fail') }],
  otherwise,
  promotedStatePaths: [],
});

test('grouping verifiers preserves outcome routing later in the full continuation', () => {
  const a = {
    ...independent('a'),
    kind: 'verifier',
    signals: { verdict: ['accepted', 'rejected'] },
    diagnostic: { kind: 'null' },
  };
  const b = { ...independent('b'), kind: 'verifier', signals: {}, diagnostic: { kind: 'null' } };
  const route = decision('acceptance', verdict('a'));
  const source = doc(sequence('root', [a, b, independent('later'), route]));
  const before = structuredClone(source);
  const grouped = wrapParallelActivities(source, 'a', ['b']);
  assert.equal(automaticProtectionTarget(grouped.document, grouped.name), undefined);
  assert.deepEqual(findNode(grouped.document.graph.root, 'acceptance'), route);
  assert.deepEqual(source, before);
});

test('a mixed immediate decision is not mistaken for a refreshable standard error checkpoint', () => {
  const route = decision('result', workerError('writer'));
  route.branches.push({ when: verdict('writer'), node: independent('publish') });
  const source = doc(sequence('root', [independent('writer'), route]));
  assert.equal(automaticProtectionTarget(source, 'writer'), undefined);
});

test('only a sole compatible checkpoint can refresh, without custom handling in its continuation', () => {
  const group: GraphNode = {
    kind: 'par',
    name: 'reviews',
    state: schema,
    branches: [independent('a'), independent('b')],
    join: { kind: 'all' },
    promotedStatePaths: [],
  };
  const checkpoint = decision('on_error', workerError('a'));
  const source = doc(sequence('root', [group, checkpoint]));
  assert.equal(automaticProtectionTarget(source, 'reviews'), 'reviews');
  checkpoint.otherwise = sequence('continuation', [
    independent('later'),
    decision('accepted', verdict('a')),
  ]);
  assert.equal(automaticProtectionTarget(source, 'reviews'), undefined);
  checkpoint.otherwise = terminal();
  source.graph.root.children.push(independent('after_checkpoint'));
  assert.equal(automaticProtectionTarget(source, 'reviews'), undefined);
});

test('custom partial errors and mixed unrelated sources cannot refresh an overlapping checkpoint', () => {
  const route = decision('result', { ...workerError('writer'), labels: ['crash'] });
  const source = doc(sequence('root', [independent('writer'), route]));
  assert.equal(automaticProtectionTarget(source, 'writer'), undefined);
  route.branches[0].when = { kind: 'any', guards: [workerError('writer'), workerError('other')] };
  assert.equal(automaticProtectionTarget(source, 'writer'), undefined);
});

test('full continuation scans nested choices, loop exits and first-join controls', () => {
  const controls: GraphNode[] = [
    sequence('nested', [independent('later'), decision('route', verdict('writer'))]),
    {
      kind: 'loop',
      name: 'retry',
      state: schema,
      body: independent('later'),
      maxIterations: 3,
      until: { kind: 'not', guard: verdict('writer') },
      promotedStatePaths: [],
    },
    {
      kind: 'par',
      name: 'race',
      state: schema,
      branches: [independent('later')],
      join: {
        kind: 'first',
        when: { kind: 'k_of_n', count: 1, values: [verdict('writer').value], labels: ['accepted'] },
      },
      promotedStatePaths: [],
    },
  ];
  for (const control of controls) {
    const source = doc(sequence('root', [independent('writer'), independent('between'), control]));
    assert.equal(automaticProtectionTarget(source, 'writer'), undefined);
  }
});

test('a later decision about a different worker still permits protection for the new activity', () => {
  const source = doc(
    sequence('root', [
      independent('writer'),
      independent('other'),
      decision('route', verdict('other')),
    ])
  );
  assert.equal(automaticProtectionTarget(source, 'writer'), 'writer');
});

test('Choice protection refresh respects branch masks and notices router outcome references in the suffix', () => {
  const when = verdict('router');
  const choice: GraphNode = {
    kind: 'choice',
    name: 'choose',
    state: schema,
    branches: [{ when, node: independent('left') }],
    otherwise: independent('right'),
    promotedStatePaths: [],
  };
  const checkpoint = decision('on_error', {
    kind: 'any',
    guards: [
      { kind: 'all', guards: [when, workerError('left')] },
      { kind: 'all', guards: [{ kind: 'not', guard: when }, workerError('right')] },
    ],
  });
  const source = doc(sequence('root', [independent('router'), choice, checkpoint]));
  assert.equal(automaticProtectionTarget(source, 'choose'), 'choose');
  choice.branches.push({ when: verdict('second_router'), node: independent('middle') });
  assert.equal(automaticProtectionTarget(source, 'choose'), 'choose');
  checkpoint.otherwise = sequence('continuation', [
    independent('later'),
    decision('custom', verdict('router')),
  ]);
  assert.equal(automaticProtectionTarget(source, 'choose'), undefined);
  source.graph.root.children.splice(
    2,
    1,
    independent('between'),
    decision('custom', verdict('router'))
  );
  assert.equal(automaticProtectionTarget(source, 'choose'), undefined);
});
