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

test('par branch terminals settle only their own branch visits, in every round', () => {
  const rounds = graph({
    name: 'run',
    kind: 'seq',
    children: [
      { name: 'review', kind: 'verifier' },
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
  // Both branches end at terminals, so each join is unreachable: both rounds and the run finish in
  // the reduction after the review.
  const round = (number: number, node: string, state: ControlRecord['state'], extra = {}) =>
    visit('v2:3', node, state, { visitId: `${node}-${number}`, ...extra });
  const control = [
    visit('v2:1', 'run', 'entered'),
    visit('v2:3', 'run', 'completed'),
    visit('v2:3', 'rounds', 'completed'),
    ...[1, 2].flatMap((number) => [
      round(number, 'lanes', 'completed', { detail: 'quorum_unreachable' }),
      round(number, 'left', 'completed', { branch: 'left_end' }),
      round(number, 'left_end', 'succeeded', { output: null }),
      round(number, 'right', 'completed', { branch: 'right_end' }),
      round(number, 'right_end', 'failed', { detail: 'right_end' }),
    ]),
    visit('v2:3', 'done', 'succeeded', { output: null }),
  ];
  const { nodes, controls } = projectHistory(rounds, events, 2, control);
  const states = (node: string) =>
    controls.filter((visit) => visit.node === node).map((visit) => visit.state);
  assert.equal(nodes.run.state, 'succeeded');
  assert.equal(nodes.rounds.state, 'recorded');
  assert.equal(nodes.lanes.state, 'recorded');
  assert.deepEqual(states('left'), ['succeeded', 'succeeded']);
  assert.deepEqual(states('right'), ['failed', 'failed']);
});

test('a terminal settles only the visits of its own map item', () => {
  const items = graph({
    name: 'run',
    kind: 'seq',
    children: [
      {
        name: 'items',
        kind: 'map',
        body: {
          name: 'item',
          kind: 'seq',
          children: [
            { name: 'work', kind: 'step' },
            { name: 'item_done', kind: 'succeed' },
          ],
        },
      },
      { name: 'empty_done', kind: 'succeed' },
    ],
  });
  const reference = (execution: string) => ({ node: 'work', execution, nodeInstance: execution });
  const started = (execution: string, item: number) => ({
    kind: 'node_started',
    reference: reference(execution),
    occurrence: { node: 'work', mapIndices: [item] },
  });
  const ledger: HistoryEvent['event'][] = [
    { kind: 'run_started' },
    started('1', 0),
    started('2', 1),
    {
      kind: 'node_completed',
      completion: { reference: reference('1'), outcome: { status: 'verified', output: null } },
    },
    { kind: 'terminal', result: { status: 'succeeded', output: null } },
  ];
  const history = ledger.map((event, index) => ({ cursor: `v2:${index + 1}`, event }));
  // Item 0 reaches item_done while item 1 still works, so the reducer stops item 1's visit.
  const control = [
    visit('v2:1', 'run', 'entered'),
    visit('v2:1', 'items', 'entered'),
    visit('v2:1', 'item', 'entered', { visitId: 'item-0', mapIndices: [0] }),
    visit('v2:1', 'item', 'entered', { visitId: 'item-1', mapIndices: [1] }),
    visit('v2:4', 'run', 'completed'),
    visit('v2:4', 'items', 'completed'),
    visit('v2:4', 'item', 'completed', { visitId: 'item-0', mapIndices: [0] }),
    visit('v2:4', 'item_done', 'succeeded', { mapIndices: [0], output: null }),
    visit('v2:4', 'item', 'stopped', { visitId: 'item-1', mapIndices: [1] }),
  ];
  const final = projectHistory(items, history, 4, control);
  assert.equal(final.nodes.items.state, 'succeeded');
  assert.deepEqual(
    final.controls.filter((visit) => visit.node === 'item').map((visit) => visit.state),
    ['succeeded', 'stopped']
  );
});
