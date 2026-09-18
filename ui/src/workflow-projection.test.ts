import test from 'node:test';
import assert from 'node:assert/strict';
import type { Document, GraphNode } from './domain';
import { guardLabel, projectWorkflow, type WorkflowProjection } from './workflow-projection';

const state = {
  kind: 'record',
  fields: { feedback: { type: { kind: 'string' }, required: true } },
};
const agent = (name: string): GraphNode => ({ kind: 'step', name });
const verifier = (name: string): GraphNode => ({ kind: 'verifier', name });
const fail = (name: string): GraphNode => ({ kind: 'fail', name, reason: 'failed' });
const done = (name: string): GraphNode => ({
  kind: 'succeed',
  name,
  output: { kind: 'null' },
  bindings: [],
});
const seq = (name: string, children: GraphNode[]): GraphNode => ({
  kind: 'seq',
  name,
  children,
  state,
  promotedStatePaths: [['feedback']],
  future: { retained: true },
});
const signal = (name: string, label = 'accepted') => ({
  kind: 'in',
  value: { name, source: 'signal', field: 'verdict' },
  labels: [label],
});
const error = (name: string) => ({
  kind: 'in',
  value: { name, source: 'error', field: null },
  labels: ['timeout', 'crash', 'malformed', 'refusal'],
});
function doc(root: GraphNode): Document {
  return {
    name: 'workflow',
    graph: { profile: 'openengine.graph.full/v1', root, initialInput: state, policy: {} },
    runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
  };
}
function byOwner(view: WorkflowProjection, owner: string, role?: string) {
  const node = view.nodes.find((entry) => entry.owner === owner && (!role || entry.role === role));
  assert.ok(node, `${owner} ${role ?? ''} is projected`);
  return node;
}
const outgoing = (view: WorkflowProjection, id: string) =>
  view.edges.filter((edge) => edge.source === id);

function codeChange(): Document {
  const reviews: GraphNode = {
    kind: 'par',
    name: 'parallel_reviews',
    state,
    promotedStatePaths: [['feedback']],
    join: { kind: 'all' },
    branches: [verifier('acceptance'), verifier('code')],
  };
  const decision: GraphNode = {
    kind: 'choice',
    name: 'review_result',
    state,
    promotedStatePaths: [],
    branches: [
      {
        when: { kind: 'any', guards: [error('acceptance'), error('code')] },
        node: fail('review_failed'),
      },
      { when: { kind: 'all', guards: [signal('acceptance'), signal('code')] }, node: done('done') },
    ],
    otherwise: agent('review_repair'),
  };
  const loop: GraphNode = {
    kind: 'loop',
    name: 'change_loop',
    state,
    promotedStatePaths: [['feedback']],
    maxIterations: 10,
    body: seq('change_iteration', [reviews, decision]),
  };
  const choice: GraphNode = {
    kind: 'choice',
    name: 'worker_result',
    state,
    promotedStatePaths: [],
    branches: [{ when: error('worker'), node: fail('worker_failed') }],
    otherwise: seq('changes', [loop, fail('exhausted')]),
  };
  return doc(seq('run', [agent('worker'), choice]));
}

test('canonical code-change projection flattens sequences and loops back to reviews, not the initial worker', () => {
  const document = codeChange();
  const before = structuredClone(document);
  const view = projectWorkflow(document);
  for (const hidden of [
    'run',
    'changes',
    'change_iteration',
    'worker_result',
    'worker_failed',
    'review_failed',
  ])
    assert.equal(
      view.nodes.some((node) => node.owner === hidden),
      false
    );
  const worker = byOwner(view, 'worker');
  const loopStart = byOwner(view, 'change_loop', 'loop-start');
  assert.ok(
    view.edges.some(
      (edge) => edge.source === worker.id && edge.target === loopStart.id && edge.owner === 'run'
    )
  );
  const fork = byOwner(view, 'parallel_reviews', 'fork');
  const end = byOwner(view, 'change_loop', 'loop-end');
  const repeats = view.edges.filter((edge) => edge.repeat);
  assert.equal(repeats.length, 1);
  assert.equal(repeats[0].source, end.id);
  assert.equal(repeats[0].target, fork.id);
  assert.match(repeats[0].label ?? '', /max 10/);
  assert.ok(
    view.edges.some(
      (edge) =>
        edge.source === end.id &&
        edge.target === byOwner(view, 'exhausted').id &&
        /10 rounds/.test(edge.label ?? '')
    )
  );
  for (const terminal of view.nodes.filter((node) => ['succeed', 'fail'].includes(node.kind)))
    assert.deepEqual(outgoing(view, terminal.id), []);
  const parallel = view.regions.find((region) => region.owner === 'parallel_reviews')!;
  const loop = view.regions.find((region) => region.owner === 'change_loop')!;
  assert.ok(parallel.nodeIds.every((id) => loop.nodeIds.includes(id)));
  assert.deepEqual(document, before);
});

test('choice edges retain ordered branch indexes, guard labels, and a distinct otherwise route', () => {
  const view = projectWorkflow(codeChange());
  const decision = byOwner(view, 'review_result', 'decision');
  assert.match(decision.detail ?? '', /First matching/);
  const edges = outgoing(view, decision.id);
  assert.deepEqual(
    edges.map((edge) => edge.branchIndex),
    [1, undefined]
  );
  assert.ok(edges.every((edge) => edge.owner === 'review_result'));
  assert.match(edges[0].label ?? '', /^2\. .*accepted/);
  assert.equal(edges[1].otherwise, true);
  assert.match(edges[1].label ?? '', /^OTHERWISE/);
});

test('terminal-only loop bodies have no loop end, repeat, or invented continuation', () => {
  const body = seq('body', [agent('work'), done('finish')]);
  const loop: GraphNode = { kind: 'loop', name: 'once', body, maxIterations: 10 };
  const document = doc(seq('root', [loop, agent('unreachable')]));
  const view = projectWorkflow(document);
  assert.equal(
    view.nodes.some((node) => node.role === 'loop-end'),
    false
  );
  assert.equal(
    view.edges.some((edge) => edge.repeat),
    false
  );
  assert.deepEqual(outgoing(view, byOwner(view, 'finish').id), []);
  assert.equal(
    view.edges.some((edge) => edge.target === byOwner(view, 'unreachable').id),
    false
  );
  const collapsed = projectWorkflow(document, new Set(['once']));
  assert.deepEqual(outgoing(collapsed, byOwner(collapsed, 'once', 'collapsed').id), []);
});

test('collapsed groups retain structural terminality and exact authored owners', () => {
  const document = codeChange();
  const before = structuredClone(document);
  const folded = projectWorkflow(document, new Set(['changes']));
  const subprocess = byOwner(folded, 'changes', 'collapsed');
  assert.equal(subprocess.kind, 'seq');
  assert.deepEqual(outgoing(folded, subprocess.id), []);
  assert.equal(
    folded.nodes.some((node) => node.owner === 'review_repair'),
    false
  );
  const local = projectWorkflow(document, new Set(['review_result']));
  const result = byOwner(local, 'review_result', 'collapsed');
  assert.ok(
    outgoing(local, result.id).some(
      (edge) => edge.target === byOwner(local, 'change_loop', 'loop-end').id
    )
  );
  assert.deepEqual(document, before);
});

test('collapsed loops retain round bounds and the expanded bounded continuation label', () => {
  for (const until of [undefined, signal('review')]) {
    const loop: GraphNode = {
      kind: 'loop',
      name: 'bounded_review',
      maxIterations: 10,
      body: verifier('review'),
      ...(until ? { until } : {}),
    };
    const document = doc(seq('root', [loop, fail('exhausted')]));
    const before = structuredClone(document);
    const expanded = projectWorkflow(document);
    const collapsed = projectWorkflow(document, new Set(['bounded_review']));
    const card = byOwner(collapsed, 'bounded_review', 'collapsed');
    assert.match(card.detail ?? '', /Up to 10 rounds.*collapsed/);
    if (until) assert.match(card.detail ?? '', /Stop when.*accepted/);
    const fullExit = outgoing(expanded, byOwner(expanded, 'bounded_review', 'loop-end').id).find(
      (edge) => !edge.repeat
    )!;
    const collapsedExit = outgoing(collapsed, card.id)[0];
    assert.equal(collapsedExit.label, fullExit.label);
    assert.equal(
      collapsedExit.label,
      until ? 'Until met or 10 rounds complete' : 'After 10 rounds'
    );
    assert.equal(collapsedExit.owner, 'bounded_review');
    assert.deepEqual(document, before);
  }
});

test('parallel joins show their actual rule and never connect terminal activities into a join', () => {
  function parallel(join: any): GraphNode {
    return { kind: 'par', name: 'parallel', branches: [fail('stop'), agent('work')], join };
  }
  const all = projectWorkflow(doc(parallel({ kind: 'all' })));
  assert.equal(byOwner(all, 'parallel', 'join').label, 'Parallel group ends');
  assert.deepEqual(outgoing(all, byOwner(all, 'stop').id), []);
  assert.ok(all.edges.some((edge) => edge.secondary && /join not reached/.test(edge.label ?? '')));
  const any = projectWorkflow(doc(parallel({ kind: 'any' })));
  assert.match(byOwner(any, 'parallel', 'join').label, /Any one branch/);
  assert.deepEqual(outgoing(any, byOwner(any, 'stop').id), []);
  assert.equal(
    any.edges.filter((edge) => edge.target === byOwner(any, 'parallel', 'join').id).length,
    1
  );
  const quorum = projectWorkflow(doc(parallel({ kind: 'quorum', count: 2 })));
  assert.equal(byOwner(quorum, 'parallel', 'join').label, 'Parallel group ends');
  const first = projectWorkflow(doc(parallel({ kind: 'first', when: signal('work') })));
  assert.match(byOwner(first, 'parallel', 'join').label, /First matching branch/);
  assert.match(byOwner(first, 'parallel', 'join').detail ?? '', /without a matching branch/);
});

test('loop labels retain until conditions and round bounds; a one-round loop has no retry edge', () => {
  const loop: GraphNode = {
    kind: 'loop',
    name: 'review_loop',
    body: verifier('review'),
    maxIterations: 3,
    until: signal('review'),
  };
  const view = projectWorkflow(doc(seq('root', [loop, done('complete')])));
  assert.match(byOwner(view, 'review_loop', 'loop-start').detail ?? '', /Up to 3 rounds.*accepted/);
  const repeat = view.edges.find((edge) => edge.repeat)!;
  assert.equal(repeat.target, byOwner(view, 'review').id);
  assert.match(repeat.label ?? '', /Until not met/);
  const exit = view.edges.find((edge) => edge.target === byOwner(view, 'complete').id)!;
  assert.match(exit.label ?? '', /Until met or 3 rounds complete/);
  const once = projectWorkflow(doc({ ...loop, maxIterations: 1 }));
  assert.equal(
    once.edges.some((edge) => edge.repeat),
    false
  );
});

test('maps identify separate item state, bounded item repetition, and empty/overflow bypasses', () => {
  const map: GraphNode = {
    kind: 'map',
    name: 'each_task',
    over: { source: 'item', path: ['task.list'] },
    maxItems: 5,
    body: agent('work'),
  };
  const view = projectWorkflow(doc(seq('root', [map, done('finish')])));
  assert.match(
    byOwner(view, 'each_task', 'map-start').detail ?? '',
    /Item · task.list.*Up to 5 items/
  );
  assert.match(view.edges.find((edge) => edge.repeat)?.label ?? '', /independently/);
  const bypass = view.edges.find(
    (edge) =>
      edge.source === byOwner(view, 'each_task', 'map-start').id &&
      edge.target === byOwner(view, 'each_task', 'map-end').id
  )!;
  assert.equal(bypass.secondary, true);
  assert.match(bypass.label ?? '', /No items or item limit exceeded/);
  const terminalMap = { ...map, body: done('item_terminal') };
  const terminal = projectWorkflow(doc(seq('root', [terminalMap, agent('after_empty_map')])));
  assert.deepEqual(outgoing(terminal, byOwner(terminal, 'item_terminal').id), []);
  assert.equal(
    terminal.edges.some((edge) => edge.repeat),
    false
  );
  assert.ok(terminal.edges.some((edge) => edge.target === byOwner(terminal, 'after_empty_map').id));
  const collapsed = projectWorkflow(
    doc(seq('root', [terminalMap, agent('after_empty_map')])),
    new Set(['each_task'])
  );
  assert.equal(outgoing(collapsed, byOwner(collapsed, 'each_task', 'collapsed').id).length, 1);
});

test('root activities, empty draft groups, and unknown nodes remain selectable', () => {
  const root = projectWorkflow(doc(agent('solo')));
  assert.equal(root.nodes.length, 1);
  assert.equal(root.nodes[0].owner, 'solo');
  assert.deepEqual(root.edges, []);
  for (const node of [
    seq('empty_sequence', []),
    { kind: 'choice', name: 'empty_choice', branches: [] },
    { kind: 'par', name: 'empty_parallel', branches: [], join: { kind: 'all' } },
  ]) {
    const view = projectWorkflow(doc(node));
    assert.equal(view.nodes.length, 1);
    assert.equal(view.nodes[0].role, 'empty');
    assert.equal(view.nodes[0].owner, node.name);
    assert.match(view.nodes[0].detail ?? '', /Draft/);
  }
  const unknown = projectWorkflow(doc({ kind: 'future', name: 'future_node', retained: true }));
  assert.equal(unknown.nodes[0].kind, 'future');
  assert.match(unknown.nodes[0].detail ?? '', /Advanced node/);
});

test('all IDs are prefixed and unique, including synthetic nodes resembling authored names', () => {
  const graph = seq('workflow', [
    {
      kind: 'par',
      name: 'join',
      join: { kind: 'all' },
      branches: [agent('join'), agent('workflow:virtual:join:join')],
    },
    done('end'),
  ]);
  const view = projectWorkflow(doc(graph));
  const ids = [...view.nodes, ...view.edges, ...view.regions].map((entry) => entry.id);
  assert.equal(new Set(ids).size, ids.length);
  assert.ok(ids.every((id) => id.startsWith('workflow:')));
  const nodes = new Set(view.nodes.map((node) => node.id));
  assert.ok(view.edges.every((edge) => nodes.has(edge.source) && nodes.has(edge.target)));
  assert.ok(view.regions.every((region) => region.nodeIds.every((id) => nodes.has(id))));
});

test('guard labels preserve grouping and remain bounded and safe for malformed imports', () => {
  assert.match(
    guardLabel({
      kind: 'all',
      guards: [signal('a'), { kind: 'not', guard: signal('b', 'rejected') }],
    }),
    /AND.*NOT/
  );
  assert.match(
    guardLabel({
      kind: 'k_of_n',
      count: 2,
      values: [signal('a').value, signal('b').value],
      labels: ['accepted'],
    }),
    /At least 2/
  );
  assert.match(
    guardLabel({ kind: 'k_of_map', count: 3, value: signal('review').value, labels: ['accepted'] }),
    /At least 3 items/
  );
  assert.equal(guardLabel({ kind: 'future' }), 'Advanced condition');
  assert.doesNotThrow(() =>
    guardLabel({ kind: 'in', value: { name: { toString: null } }, labels: [{ toString: null }] })
  );
  const cyclic: any = { kind: 'not' };
  cyclic.guard = cyclic;
  assert.ok(guardLabel(cyclic).length <= 180);
  assert.ok(
    guardLabel({ kind: 'any', guards: Array.from({ length: 50 }, () => error('long_name')) })
      .length <= 180
  );
});

test('malformed imported UTF-16 names remain intact and cannot break node or region IDs', () => {
  const name = String.fromCharCode(0xd800);
  const loop: GraphNode = { kind: 'loop', name, maxIterations: 2, body: agent(`${name}_worker`) };
  const document = doc(loop);
  const before = structuredClone(document);
  const view = projectWorkflow(document);
  assert.equal(view.regions[0].owner, name);
  assert.equal(byOwner(view, name, 'loop-start').owner, name);
  assert.ok([...view.nodes, ...view.regions].every((entry) => entry.id.startsWith('workflow:')));
  assert.deepEqual(document, before);
});

test('review-loop continuation says Approved only when other exits fail', () => {
  const loop: GraphNode = {
    kind: 'loop',
    name: 'review_rounds',
    maxIterations: 3,
    until: { kind: 'any', guards: [signal('review'), error('review')] },
    body: seq('round', [
      agent('draft'),
      { ...verifier('review'), signals: { verdict: ['accepted', 'rejected'] } },
    ]),
    state,
  };
  const checkpoint: GraphNode = {
    kind: 'choice',
    name: 'review_result',
    state,
    branches: [
      { when: error('review'), node: fail('execution_error') },
      {
        when: {
          kind: 'in',
          value: { name: 'review_rounds', source: 'group', field: 'terminated' },
          labels: ['exhausted'],
        },
        node: fail('exhausted'),
      },
    ],
    otherwise: done('finished'),
  };
  const document = doc(seq('root', [loop, checkpoint]));
  const view = projectWorkflow(document);
  assert.ok(view.edges.some((edge) => edge.owner === 'review_rounds' && edge.label === 'Approved'));
  assert.ok(
    view.edges.some((edge) => edge.repeat && edge.label === 'Try again · up to 3 attempts')
  );
  const custom = structuredClone(document);
  custom.graph.root.children[0].until.guards.push(signal('review', 'rejected'));
  assert.equal(
    projectWorkflow(custom).edges.some((edge) => edge.label === 'Approved'),
    false
  );
});
