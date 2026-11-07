import test from 'node:test';
import assert from 'node:assert/strict';
import {
  appendControlHistory,
  controlVisitLabel,
  projectControlHistory,
  type ControlRecord,
} from './control-history';
import {
  historyMoments,
  nodeReplayPositions,
  projectHistory,
  type HistoryEvent,
  type HistoryPage,
} from './run-history';
import type { Document } from './domain';

const doc: Document = {
  name: 'control-test',
  graph: {
    profile: 'full-v1',
    initialInput: { kind: 'null' },
    policy: {},
    root: {
      name: 'run',
      kind: 'seq',
      children: [
        { name: 'review', kind: 'verifier' },
        {
          name: 'review_result',
          kind: 'choice',
          branches: [{ node: { name: 'done', kind: 'succeed' }, when: {} }],
          otherwise: { name: 'repair', kind: 'step' },
        },
      ],
    },
  },
  runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
};
const events: HistoryEvent[] = [
  { cursor: 'v2:1', event: { kind: 'run_started' } },
  {
    cursor: 'v2:2',
    event: {
      kind: 'node_started',
      reference: { node: 'review', execution: '1', nodeInstance: '1' },
    },
  },
  {
    cursor: 'v2:3',
    event: {
      kind: 'node_completed',
      completion: {
        reference: { node: 'review', execution: '1', nodeInstance: '1' },
        outcome: { status: 'verified', signals: { verdict: 'accepted' } },
      },
    },
  },
  { cursor: 'v2:4', event: { kind: 'terminal', result: { status: 'succeeded', output: null } } },
];
const records: ControlRecord[] = [
  { cursor: 'v2:1', node: 'run', visitId: 'root', mapIndices: [], state: 'entered' },
  {
    cursor: 'v2:4',
    node: 'review_result',
    visitId: 'route',
    mapIndices: [],
    state: 'succeeded',
    branch: 'done',
  },
  {
    cursor: 'v2:4',
    node: 'done',
    visitId: 'finish',
    mapIndices: [],
    state: 'succeeded',
    output: { report: 'report.md' },
  },
  { cursor: 'v2:4', node: 'run', visitId: 'root', mapIndices: [], state: 'succeeded' },
];
const page = (events: HistoryEvent[], control: ControlRecord[]): HistoryPage => ({
  events,
  control,
  nextCursor: events.at(-1)!.cursor,
  headCursor: 'v2:4',
  complete: events.at(-1)!.cursor === 'v2:4',
});

test('a terminal-only choice gets visits without fabricating a worker execution', () => {
  const final = projectHistory(doc, events, 3, records);
  assert.equal(final.invocations.length, 1);
  assert.equal(final.controls.find((visit) => visit.node === 'review_result')?.branch, 'done');
  assert.deepEqual(final.nodes.done, {
    state: 'succeeded',
    count: 1,
    countUnit: 'visit',
    detail: undefined,
  });
  assert.equal(final.nodes.review_result.countUnit, 'visit');
  const earlier = projectHistory(doc, events, 1, records);
  assert.equal(earlier.controls.length, 1);
  assert.equal(earlier.nodes.done, undefined);
  assert.equal(earlier.nodes.review_result, undefined);
  assert.equal(projectHistory(doc, events, -1, records).controls.length, 0);
  assert.deepEqual(nodeReplayPositions(doc, historyMoments(events), events, 'done', records), [4]);
});

test('paging and identical retransmission preserve one visit with its latest prefix state', () => {
  const first = page(events.slice(0, 2), records.slice(0, 1));
  const second = page(events.slice(2), records.slice(1));
  const merged = appendControlHistory(appendControlHistory([], first), second);
  assert.deepEqual(appendControlHistory(merged, second), records);
  const projected = projectControlHistory(merged, events, 3);
  assert.equal(projected.length, 3);
  assert.equal(projected[0].start, 0);
  assert.equal(projected[0].updated, 3);
  assert.equal(projected[0].state, 'succeeded');
  assert.equal(projectControlHistory(merged, events, 2)[0].state, 'entered');
  assert.throws(() => appendControlHistory(merged, { ...second, control: [] }), /inconsistent/);
  assert.throws(() => appendControlHistory([], { ...first, control: records }), /inconsistent/);
});

test('loop identities remain separate and late updates do not reset visit numbering', () => {
  const visits: ControlRecord[] = [
    { ...records[0], node: 'loop_body', visitId: 'first' },
    { ...records[0], cursor: 'v2:2', node: 'loop_body', visitId: 'second' },
    { ...records[0], cursor: 'v2:3', node: 'loop_body', visitId: 'first', state: 'completed' },
    { ...records[0], cursor: 'v2:4', node: 'loop_body', visitId: 'third' },
  ];
  assert.deepEqual(
    projectControlHistory(visits, events, 3).map((visit) => visit.visit),
    [1, 2, 3]
  );
  assert.equal(
    controlVisitLabel({ ...projectControlHistory(visits, events, 3)[2], mapIndices: [0, 2] }),
    'Map 1, item 1 · Map 2, item 3 · Visit 3'
  );
});

test('a retransmission cannot introduce earlier control history or change a visit identity', () => {
  const completed: ControlRecord = { ...records[0], cursor: 'v2:2', state: 'completed' };
  assert.throws(
    () =>
      appendControlHistory(
        [completed],
        page(events.slice(0, 2), [records[0], completed]),
        events.slice(0, 2)
      ),
    /inconsistent/
  );
  assert.throws(
    () =>
      appendControlHistory(
        [records[0]],
        page(events.slice(2), [{ ...completed, cursor: 'v2:3', node: 'different' }])
      ),
    /inconsistent/
  );
});

test('a completed map item does not conceal another active occurrence of the same control', () => {
  const visits: ControlRecord[] = [
    { ...records[0], node: 'review_result', visitId: 'item0', mapIndices: [0] },
    { ...records[0], node: 'review_result', visitId: 'item1', mapIndices: [1], state: 'completed' },
  ];
  assert.equal(projectHistory(doc, events, 0, visits).nodes.review_result.state, 'running');
  assert.equal(projectHistory(doc, events, 0, visits).nodes.review_result.count, 2);
  const stopped: ControlRecord[] = [...visits, { ...visits[0], cursor: 'v2:2', state: 'stopped' }];
  assert.equal(projectHistory(doc, events, 1, stopped).nodes.review_result.state, 'skipped');
});

const visit = (
  cursor: string,
  node: string,
  state: ControlRecord['state'],
  extra: Partial<ControlRecord> = {}
): ControlRecord => ({ cursor, node, visitId: node, mapIndices: [], state, ...extra });
const graph = (root: Document['graph']['root']): Document => ({
  ...doc,
  graph: { ...doc.graph, root },
});
const fail = (name: string) => ({ name, kind: 'fail', reason: name });

test('a terminal settles only the visits it ended, in its own loop iteration', () => {
  const loop = graph({
    name: 'run',
    kind: 'seq',
    children: [
      {
        name: 'rounds',
        kind: 'loop',
        maxIterations: 2,
        body: {
          name: 'round',
          kind: 'choice',
          branches: [{ node: fail('stop'), when: {} }],
          otherwise: { name: 'review', kind: 'verifier' },
        },
      },
    ],
  });
  const failed: HistoryEvent[] = [
    ...events.slice(0, 3),
    { cursor: 'v2:4', event: { kind: 'terminal', result: { status: 'failed', reason: 'stop' } } },
  ];
  // The review ends round 1, and round 2 selects stop within the same reduction.
  const control = [
    visit('v2:1', 'run', 'entered'),
    visit('v2:1', 'rounds', 'entered'),
    visit('v2:1', 'round', 'entered', { visitId: 'round-1', branch: 'review' }),
    visit('v2:3', 'run', 'completed'),
    visit('v2:3', 'rounds', 'completed'),
    visit('v2:3', 'round', 'completed', { visitId: 'round-1', branch: 'review' }),
    visit('v2:3', 'round', 'completed', { visitId: 'round-2', branch: 'stop' }),
    visit('v2:3', 'stop', 'failed', { detail: 'stop' }),
  ];
  const final = projectHistory(loop, failed, 3, control);
  assert.equal(final.nodes.rounds.state, 'failed');
  assert.equal(final.nodes.round.state, 'failed');
  assert.deepEqual(
    final.controls.filter((visit) => visit.node === 'round').map((visit) => visit.state),
    ['completed', 'failed']
  );
  assert.equal(projectHistory(loop, failed, 2, control).nodes.run.state, 'failed');
});

test('a par branch terminal never settles the par or the groups above it', () => {
  const items = graph({
    name: 'run',
    kind: 'seq',
    children: [
      { name: 'review', kind: 'verifier' },
      {
        name: 'items',
        kind: 'map',
        body: {
          name: 'lanes',
          kind: 'par',
          join: { kind: 'all' },
          branches: [
            {
              name: 'left',
              kind: 'choice',
              branches: [{ node: { name: 'left_end', kind: 'succeed' }, when: {} }],
              otherwise: { name: 'left_work', kind: 'step' },
            },
            {
              name: 'right',
              kind: 'choice',
              branches: [{ node: fail('right_end'), when: {} }],
              otherwise: { name: 'right_work', kind: 'step' },
            },
          ],
        },
      },
      { name: 'done', kind: 'succeed' },
    ],
  });
  // In both map items, both branches end at terminals: each join is unreachable, so the run
  // continues to done within the same reduction.
  const item = (index: number, node: string, state: ControlRecord['state'], extra = {}) =>
    visit('v2:3', node, state, { visitId: `${node}-${index}`, mapIndices: [index], ...extra });
  const control = [
    visit('v2:1', 'run', 'entered'),
    visit('v2:3', 'run', 'completed'),
    visit('v2:3', 'items', 'completed'),
    ...[0, 1].flatMap((index) => [
      item(index, 'lanes', 'completed', { detail: 'quorum_unreachable' }),
      item(index, 'left', 'completed', { branch: 'left_end' }),
      item(index, 'left_end', 'succeeded', { output: null }),
      item(index, 'right', 'completed', { branch: 'right_end' }),
      item(index, 'right_end', 'failed', { detail: 'right_end' }),
    ]),
    visit('v2:3', 'done', 'succeeded', { output: null }),
  ];
  const { nodes } = projectHistory(items, events, 2, control);
  assert.equal(nodes.run.state, 'succeeded');
  assert.equal(nodes.items.state, 'recorded');
  assert.equal(nodes.lanes.state, 'recorded');
  assert.equal(nodes.left.state, 'succeeded');
  assert.equal(nodes.right.state, 'failed');
});

test('a branch terminal never settles a later visit of its branch', () => {
  const rounds = graph({
    name: 'run',
    kind: 'seq',
    children: [
      {
        name: 'rounds',
        kind: 'loop',
        maxIterations: 2,
        body: {
          name: 'lanes',
          kind: 'par',
          join: { kind: 'all' },
          branches: [
            {
              name: 'left',
              kind: 'choice',
              branches: [{ node: fail('left_end'), when: {} }],
              otherwise: { name: 'left_work', kind: 'step' },
            },
            { name: 'right_work', kind: 'step' },
          ],
        },
      },
    ],
  });
  const reference = (node: string, execution: string) => ({
    node,
    execution,
    nodeInstance: execution,
  });
  const outcome = { status: 'verified', output: null };
  const history: HistoryEvent[] = [
    { kind: 'run_started' },
    { kind: 'node_started', reference: reference('right_work', '1') },
    { kind: 'node_completed', completion: { reference: reference('right_work', '1'), outcome } },
    { kind: 'node_started', reference: reference('left_work', '2') },
    { kind: 'node_completed', completion: { reference: reference('left_work', '2'), outcome } },
  ].map((event, index) => ({ cursor: `v2:${index + 1}`, event }));
  // Round 1's left branch ends at left_end; round 2's left visit completes later through left_work.
  const control = [
    visit('v2:1', 'run', 'entered'),
    visit('v2:1', 'rounds', 'entered'),
    visit('v2:1', 'lanes', 'entered', { visitId: 'lanes-1' }),
    visit('v2:1', 'left', 'completed', { visitId: 'left-1', branch: 'left_end' }),
    visit('v2:1', 'left_end', 'failed', { detail: 'left_end' }),
    visit('v2:3', 'lanes', 'completed', { visitId: 'lanes-1', detail: 'quorum_unreachable' }),
    visit('v2:3', 'lanes', 'entered', { visitId: 'lanes-2' }),
    visit('v2:3', 'left', 'entered', { visitId: 'left-2', branch: 'left_work' }),
    visit('v2:5', 'left', 'completed', { visitId: 'left-2', branch: 'left_work' }),
  ];
  const final = projectHistory(rounds, history, 4, control);
  assert.equal(final.nodes.left.state, 'recorded');
  assert.deepEqual(
    final.controls.filter((visit) => visit.node === 'left').map((visit) => visit.state),
    ['failed', 'completed']
  );
});
