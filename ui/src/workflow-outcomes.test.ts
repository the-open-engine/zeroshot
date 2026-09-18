import test from 'node:test';
import assert from 'node:assert/strict';
import type { Document, GraphNode } from './domain';
import { analyzeWorkflowOutcomes } from './workflow-outcomes';
import { projectWorkflow } from './workflow-projection';

const work = (name: string): GraphNode => ({ kind: 'step', name });
const state = { kind: 'record', fields: { result: { required: true, type: { kind: 'string' } } } };
const seq = (name: string, children: GraphNode[]): GraphNode => ({ kind: 'seq', name, children });
const fail = (name = 'failed'): GraphNode => ({
  kind: 'fail',
  name,
  reason: 'authored_failure_reason',
});
const done = (name = 'done'): GraphNode => ({
  kind: 'succeed',
  name,
  output: state,
  bindings: [{ target: ['result'], value: { source: 'state', path: ['result'] } }],
});
const errors = (name = 'work') => ({
  kind: 'in',
  value: { name, source: 'error', field: null },
  labels: ['crash', 'malformed', 'refusal', 'timeout'],
});
const group = (name: string, field: string, label: string) => ({
  kind: 'in',
  value: { name, source: 'group', field },
  labels: [label],
});
const choice = (when: unknown, otherwise: GraphNode = done()): GraphNode => ({
  kind: 'choice',
  name: 'outcome',
  state,
  promotedStatePaths: [['result']],
  branches: [{ when, node: fail() }],
  otherwise,
});
const doc = (root: GraphNode): Document => ({
  name: 'test',
  graph: { profile: 'test', root, initialInput: state, policy: {} },
  runtime: { harness: '', provider: '', size: 'small', nodes: {} },
});

test('full worker failure becomes a policy at the exact checkpoint without rewriting any scopes or outputs', () => {
  const document = doc(seq('run', [work('work'), choice(errors())]));
  const before = structuredClone(document);
  const analysis = analyzeWorkflowOutcomes(document);
  assert.deepEqual(analysis.policies, [
    {
      owner: 'work',
      choiceName: 'outcome',
      checkpoint: 'outcome',
      branchIndex: 0,
      failName: 'failed',
      reason: 'authored_failure_reason',
      guard: errors(),
      sources: ['work'],
      kind: 'worker-error',
      boundary: 'run',
    },
  ]);
  assert.equal(analysis.policies[0].guard, document.graph.root.children[1].branches[0].when);
  const view = projectWorkflow(document);
  assert.deepEqual(
    view.nodes.map((node) => [node.owner, node.role]),
    [
      ['work', 'activity'],
      ['done', 'completion'],
    ]
  );
  assert.equal(view.nodes[1].label, 'Complete');
  assert.equal(view.edges.length, 1);
  assert.deepEqual(document, before);
});

test('partial, signal, negated, mixed, unknown and malformed guards stay visible', () => {
  const signal = {
    kind: 'in',
    value: { name: 'work', source: 'signal', field: 'verdict' },
    labels: ['rejected'],
  };
  for (const when of [
    { ...errors(), labels: ['crash'] },
    { ...errors(), labels: ['crash', 'crash', 'refusal', 'timeout'] },
    { ...errors(), labels: [...errors().labels, 'future'] },
    { ...errors(), future: true },
    { ...errors(), value: { ...errors().value, field: 'unknown' } },
    { ...errors(), value: { ...errors().value, future: true } },
    errors('missing'),
    signal,
    { kind: 'not', guard: errors() },
    { kind: 'all', guards: [errors()] },
    { kind: 'any', guards: [errors(), signal] },
    { kind: 'any', guards: [] },
    { kind: 'any', guards: [errors(), null] },
    { kind: 'future', guards: [errors()] },
    null,
  ]) {
    const document = doc(seq('run', [work('work'), choice(when)]));
    assert.deepEqual(analyzeWorkflowOutcomes(document).policies, [], JSON.stringify(when));
    assert.ok(
      projectWorkflow(document).nodes.some((node) => node.owner === 'failed'),
      JSON.stringify(when)
    );
  }
});

test('mixed business decisions retain original branch indices and authored failure paths', () => {
  const decision = choice(errors(), work('revise'));
  decision.branches.push({
    when: {
      kind: 'in',
      value: { source: 'signal', name: 'work', field: 'verdict' },
      labels: ['rejected'],
    },
    node: fail('business_rejected'),
  });
  const document = doc(seq('run', [work('work'), decision, done()]));
  const view = projectWorkflow(document);
  const decisionView = view.nodes.find((node) => node.owner === 'outcome')!;
  const paths = view.edges.filter((edge) => edge.source === decisionView.id);
  assert.deepEqual(
    paths.map((edge) => [edge.branchIndex, edge.otherwise]),
    [
      [1, undefined],
      [undefined, true],
    ]
  );
  const terminal = view.nodes.find((node) => node.owner === 'business_rejected')!;
  assert.equal(terminal.role, 'completion');
  assert.equal(
    view.edges.some((edge) => edge.source === terminal.id),
    false
  );
});

test('parallel policy association stays after the join and branch-local policies stay in their branch', () => {
  const inside: GraphNode = { ...choice(errors('a'), work('continue_a')), name: 'inside' };
  inside.branches[0].node.name = 'a_failed';
  const parallel: GraphNode = {
    kind: 'par',
    name: 'parallel',
    join: { kind: 'all' },
    branches: [seq('a_branch', [work('a'), inside]), work('b')],
  };
  const guard = { kind: 'any', guards: [errors('a'), errors('b')] };
  const document = doc(seq('run', [parallel, choice(guard)]));
  const analysis = analyzeWorkflowOutcomes(document);
  assert.deepEqual(
    analysis.policies.map((policy) => [policy.owner, policy.checkpoint]),
    [
      ['a', 'inside'],
      ['parallel', 'outcome'],
    ]
  );
  assert.equal(analysis.policies[0].boundary, 'parallel-branch');
  assert.equal(analysis.policies[0].parallelName, 'parallel');
  assert.equal(analysis.policies[1].boundary, 'run');
  const view = projectWorkflow(document);
  const join = view.nodes.find((node) => node.role === 'join')!;
  const finish = view.nodes.find((node) => node.owner === 'done')!;
  assert.ok(view.edges.some((edge) => edge.source === join.id && edge.target === finish.id));
  assert.ok(
    view.regions
      .find((region) => region.owner === 'parallel')!
      .nodeIds.includes(view.nodes.find((node) => node.owner === 'continue_a')!.id)
  );
});

test('map error thresholds and known control failures are folded without inventing new defaults', () => {
  const map: GraphNode = { kind: 'map', name: 'items', body: work('work'), maxItems: 5 };
  const threshold = { ...errors(), kind: 'k_of_map', count: 1 };
  const document = doc(
    seq('run', [
      map,
      choice({ kind: 'any', guards: [threshold, group('items', 'overflow', 'overflow')] }),
    ])
  );
  const policy = analyzeWorkflowOutcomes(document).policies[0];
  assert.equal(policy.owner, 'items');
  assert.equal(policy.kind, 'mixed-error');
  assert.deepEqual(policy.sources, ['work', 'items']);
  for (const [control, guard] of [
    [
      { kind: 'loop', name: 'rounds', body: work('work'), maxIterations: 3 },
      group('rounds', 'terminated', 'exhausted'),
    ],
    [
      { kind: 'par', name: 'reviews', branches: [work('work')], join: { kind: 'all' } },
      group('reviews', 'joined', 'quorum_unreachable'),
    ],
    [
      { kind: 'par', name: 'reviews', branches: [work('work')], join: { kind: 'first' } },
      group('reviews', 'raced', 'no_satisfier'),
    ],
  ] as [GraphNode, unknown][]) {
    assert.equal(
      analyzeWorkflowOutcomes(doc(seq('run', [control, choice(guard)]))).policies[0].owner,
      control.name
    );
  }
  for (const bad of [
    { ...threshold, count: 0 },
    { ...threshold, count: 1.5 },
    { ...threshold, future: true },
    group('items', 'overflow', 'ok'),
  ])
    assert.equal(analyzeWorkflowOutcomes(doc(seq('run', [map, choice(bad)]))).policies.length, 0);
  assert.equal(
    analyzeWorkflowOutcomes(doc(seq('run', [work('work'), choice(threshold)]))).policies.length,
    0
  );
  assert.equal(analyzeWorkflowOutcomes(doc(seq('run', [work('work'), done()]))).policies.length, 0);
});

test('nonadjacent error checks retain their choice checkpoint rather than becoming immediate worker failure', () => {
  const document = doc(seq('run', [work('work'), work('log_receipt'), choice(errors())]));
  assert.equal(analyzeWorkflowOutcomes(document).policies[0].owner, 'outcome');
  const view = projectWorkflow(document);
  assert.deepEqual(
    view.nodes.map((node) => node.owner),
    ['work', 'log_receipt', 'done']
  );
  assert.equal(view.edges.length, 2);
});

test('early terminals inside loops, maps and parallel branches stay distinct and never gain continuation edges', () => {
  for (const control of [
    { kind: 'loop', name: 'control', body: done(), maxIterations: 3 },
    { kind: 'map', name: 'control', body: done(), maxItems: 3 },
    { kind: 'par', name: 'control', branches: [done(), work('other')], join: { kind: 'all' } },
  ]) {
    const document = doc(seq('run', [control, work('after')]));
    assert.equal(analyzeWorkflowOutcomes(document).completions[0].mode, 'early');
    const view = projectWorkflow(document);
    const terminal = view.nodes.find((node) => node.owner === 'done')!;
    assert.equal(terminal.label, control.kind === 'par' ? 'End branch' : 'Finish run early');
    assert.equal(
      view.edges.some((edge) => edge.source === terminal.id),
      false
    );
  }
  const document = doc(seq('run', [choice(errors()), work('unreachable')]));
  assert.equal(analyzeWorkflowOutcomes(document).completions[0].mode, 'early');
  const view = projectWorkflow(document);
  assert.equal(
    view.edges.some(
      (edge) => edge.target === view.nodes.find((node) => node.owner === 'unreachable')!.id
    ),
    false
  );
});

test('unknown terminal extensions and ambiguous identities remain explicitly visible', () => {
  const decision = choice(errors());
  decision.branches[0].node.future = true;
  const document = doc(seq('run', [work('work'), decision]));
  assert.equal(analyzeWorkflowOutcomes(document).policies.length, 0);
  assert.ok(projectWorkflow(document).nodes.some((node) => node.owner === 'failed'));
  delete decision.branches[0].node.future;
  document.graph.root.children.unshift(work('work'));
  assert.equal(analyzeWorkflowOutcomes(document).policies.length, 0);
});

test('adjacent verifier outcomes originate directly from the verifier with original choice ownership and scopes', () => {
  const review: GraphNode = {
    kind: 'verifier',
    name: 'review',
    signals: { verdict: ['accepted', 'rejected'] },
  };
  const decision = choice(errors('review'), work('repair'));
  decision.branches.push({
    when: {
      kind: 'in',
      value: { name: 'review', source: 'signal', field: 'verdict' },
      labels: ['accepted'],
    },
    node: done(),
  });
  const document = doc(seq('run', [review, decision, work('continue_after_repair')]));
  const before = structuredClone(document);
  const view = projectWorkflow(document);
  assert.equal(
    view.nodes.some((node) => node.owner === 'outcome'),
    false
  );
  const activity = view.nodes.find((node) => node.owner === 'review')!;
  const paths = view.edges.filter((edge) => edge.source === activity.id);
  assert.deepEqual(
    paths.map((edge) => [edge.owner, edge.branchIndex, edge.otherwise]),
    [
      ['outcome', 1, undefined],
      ['outcome', undefined, true],
    ]
  );
  assert.ok(paths.every((edge) => edge.target !== edge.source));
  assert.ok(
    view.edges.some(
      (edge) =>
        edge.source === view.nodes.find((node) => node.owner === 'repair')!.id &&
        edge.target === view.nodes.find((node) => node.owner === 'continue_after_repair')!.id
    )
  );
  assert.equal(
    view.edges.some((edge) => edge.source === view.nodes.find((node) => node.owner === 'done')!.id),
    false
  );
  assert.deepEqual(document, before);
});

test('verifier outcome fusion does not cross another step, sequence scope, parallel join, or unfamiliar condition', () => {
  const review: GraphNode = {
    kind: 'verifier',
    name: 'review',
    signals: { verdict: ['accepted', 'rejected'] },
  };
  const guard = {
    kind: 'in',
    value: { name: 'review', source: 'signal', field: 'verdict' },
    labels: ['accepted'],
  };
  const decision: GraphNode = {
    kind: 'choice',
    name: 'outcome',
    branches: [{ when: guard, node: done() }],
    otherwise: work('repair'),
  };
  for (const root of [
    seq('run', [review, work('receipt'), decision]),
    seq('run', [seq('review_scope', [review]), decision]),
    seq('run', [
      { kind: 'par', name: 'parallel', branches: [review], join: { kind: 'all' } },
      decision,
    ]),
    seq('run', [
      review,
      { ...decision, branches: [{ when: { ...guard, future: true }, node: done() }] },
    ]),
    seq('run', [review, { ...decision, future: true }]),
    seq('run', [
      review,
      {
        ...decision,
        branches: [{ when: { ...guard, labels: ['unknown_verdict'] }, node: done() }],
      },
    ]),
  ])
    assert.ok(projectWorkflow(doc(root)).nodes.some((node) => node.owner === 'outcome'));
});

test('terminal-only parallel branches end locally and an unmet join still reaches the following checkpoint', () => {
  const parallel: GraphNode = {
    kind: 'par',
    name: 'parallel',
    join: { kind: 'all' },
    branches: [done('branch_complete'), fail('branch_failed')],
  };
  const document = doc(seq('run', [parallel, work('handle_group_result')]));
  const view = projectWorkflow(document);
  const end = view.nodes.find((node) => node.role === 'join')!;
  assert.ok(end);
  assert.ok(
    view.edges.some(
      (edge) =>
        edge.source === end.id &&
        edge.target === view.nodes.find((node) => node.owner === 'handle_group_result')!.id
    )
  );
  for (const terminal of view.nodes.filter((node) => node.role === 'completion')) {
    assert.equal(
      view.edges.some((edge) => edge.source === terminal.id),
      false
    );
    assert.match(terminal.detail ?? '', /branch/);
  }
  assert.equal(analyzeWorkflowOutcomes(document).completions[0].boundary, 'parallel-branch');
});

test('Choice error projection requires exact first-match route masks', () => {
  const when = {
    kind: 'in',
    value: { name: 'router', source: 'signal', field: 'path' },
    labels: ['left'],
  };
  const decision: GraphNode = {
    kind: 'choice',
    name: 'choose',
    branches: [{ when, node: work('left') }],
    otherwise: work('right'),
  };
  const masked = {
    kind: 'any',
    guards: [
      { kind: 'all', guards: [when, errors('left')] },
      { kind: 'all', guards: [{ kind: 'not', guard: when }, errors('right')] },
    ],
  };
  const document = doc(
    seq('run', [
      { kind: 'verifier', name: 'router', signals: { path: ['left', 'right'] } },
      decision,
      choice(masked),
    ])
  );
  const before = structuredClone(document);
  const policies = analyzeWorkflowOutcomes(document).policies;
  assert.equal(policies.length, 1);
  assert.equal(policies[0].owner, 'choose');
  assert.deepEqual(policies[0].sources, ['left', 'right']);
  assert.equal(
    projectWorkflow(document).nodes.some((node) => node.owner === 'failed'),
    false
  );
  assert.deepEqual(document, before);
  for (const changed of [
    { kind: 'all', guards: [errors('left'), when] },
    { kind: 'all', guards: [when, errors('right')] },
    { kind: 'all', guards: [when, { ...errors('left'), labels: ['crash'] }] },
  ]) {
    const invalid = structuredClone(document);
    invalid.graph.root.children[2].branches[0].when.guards[0] = changed;
    assert.equal(analyzeWorkflowOutcomes(invalid).policies.length, 0);
  }
});
