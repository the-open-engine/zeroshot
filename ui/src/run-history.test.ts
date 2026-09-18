import test from 'node:test';
import assert from 'node:assert/strict';
import {
  currentInvocation,
  historyMoments,
  nextReplayPosition,
  nodeReplayPositions,
  invocationLabel,
  observedRoutes,
  projectHistory,
  type HistoryEvent,
  type Outcome,
  type Reference,
} from './run-history';
import type { Document, GraphNode } from './domain';

const step = (name: string): GraphNode => ({ name, kind: 'step' });
const sequence = (name: string, ...children: GraphNode[]): GraphNode => ({
  name,
  kind: 'seq',
  children,
});
function document(root = sequence('run', step('worker'))): Document {
  return {
    name: 'test-run',
    graph: { profile: 'full-v1', initialInput: { kind: 'null' }, policy: {}, root },
    runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
  };
}
const ref = (node = 'worker', execution = '1', nodeInstance = '1'): Reference => ({
  node,
  execution,
  nodeInstance,
  runId: 'test-run',
});
function events(...values: HistoryEvent['event'][]): HistoryEvent[] {
  return values.map((event, index) => ({ cursor: `v2:${index + 1}`, event }));
}
function start(reference = ref(), attempt = 1, mapIndices: number[] = []): HistoryEvent['event'] {
  return {
    kind: 'node_started',
    reference,
    occurrence: { node: reference.node, mapIndices },
    attempt,
    input: { request: `Work ${reference.execution}` },
  };
}
function complete(
  reference = ref(),
  outcome: Outcome = { status: 'verified', output: null }
): HistoryEvent['event'] {
  return { kind: 'node_completed', completion: { reference, outcome } };
}
const log = (execution: string | null, line: string): HistoryEvent['event'] => ({
  kind: 'safe_log',
  execution,
  stream: 'output',
  line,
  timestamp: 1789000000000,
});
const success: HistoryEvent['event'] = {
  kind: 'terminal',
  result: { status: 'succeeded', output: { report: 'report.md' } },
};

// Projection is always reconstructed from the selected prefix, including when seeking backwards.
test('replay before, during and after a run never exposes future inputs, outcomes or transcripts', () => {
  const doc = document();
  const history = events(
    { kind: 'run_started' },
    start(),
    log('1', 'Working'),
    complete(ref(), { status: 'verified', output: { report: 'report.md' } }),
    log('1', 'Output drained after completion'),
    success
  );
  const before = projectHistory(doc, history, -1);
  assert.equal(before.invocations.length, 0);
  assert.equal(before.terminal, undefined);
  assert.equal(before.nodes.worker.state, 'idle');
  const finished = projectHistory(doc, history, 5);
  assert.equal(finished.terminal?.status, 'succeeded');
  assert.equal(finished.invocations[0].logs.length, 2);
  assert.equal(finished.invocations[0].end, 3);
  const earlier = projectHistory(doc, history, 1);
  assert.equal(earlier.invocations[0].state, 'running');
  assert.equal(earlier.invocations[0].outcome, undefined);
  assert.deepEqual(earlier.invocations[0].logs, []);
  assert.equal(earlier.terminal, undefined);
  assert.equal(earlier.invocations[0].end, undefined);
  const completed = projectHistory(doc, history, 3);
  assert.deepEqual(
    completed.invocations[0].logs.map((item) => item.text),
    ['Working']
  );
  assert.equal(completed.terminal, undefined);
  assert.equal(
    finished.invocations[0].logs.length,
    2,
    'seeking does not mutate a prior projection'
  );
});

test('loop visits, retries and map item occurrences retain independent identities and labels', () => {
  const doc = document(
    sequence('run', {
      kind: 'loop',
      name: 'review_rounds',
      body: { kind: 'map', name: 'items', body: step('worker') },
    })
  );
  const history = events(
    start(ref('worker', '1', '10'), 1, [0]),
    complete(ref('worker', '1', '10'), { status: 'error', code: 'crash' }),
    start(ref('worker', '2', '10'), 2, [0]),
    complete(ref('worker', '2', '10')),
    start(ref('worker', '3', '11'), 1, [1]),
    complete(ref('worker', '3', '11')),
    start(ref('worker', '4', '10'), 1, [0]),
    log('4', 'New round'),
    start(ref('worker', '5', '12'), 1, [1, 2])
  );
  const projection = projectHistory(doc, history, history.length - 1);
  assert.deepEqual(
    projection.invocations.map(({ id, instance, visit, attempt }) => [
      id,
      instance,
      visit,
      attempt,
    ]),
    [
      ['1', '10', 1, 1],
      ['2', '10', 1, 2],
      ['3', '11', 1, 1],
      ['4', '10', 2, 1],
      ['5', '12', 1, 1],
    ]
  );
  assert.equal(invocationLabel(projection.invocations[1], doc), 'Item 1 · Visit 1 · Attempt 2');
  assert.equal(invocationLabel(projection.invocations[3], doc), 'Item 1 · Visit 2');
  assert.equal(
    invocationLabel(projection.invocations[4], doc),
    'Map 1, item 2 · Map 2, item 3 · Visit 1'
  );
  assert.deepEqual(projection.invocations[1].logs, []);
  assert.equal(projection.invocations[3].logs[0].text, 'New round');
});

test('verifier rejection is a completed review, while an error is an execution failure', () => {
  const doc = document(sequence('run', { kind: 'verifier', name: 'review' }, step('repair')));
  const history = events(
    start(ref('review')),
    complete(ref('review'), {
      status: 'verifier',
      signals: { verdict: 'rejected' },
      diagnostic: { findings: ['Missing source'] },
    }),
    start(ref('repair', '2', '2')),
    complete(ref('repair', '2', '2'), {
      status: 'error',
      code: 'crash',
      reason: 'execution_failed',
    })
  );
  const projection = projectHistory(doc, history, 3);
  assert.equal(projection.nodes.review.state, 'succeeded');
  assert.equal(projection.nodes.review.detail, 'Verdict: rejected');
  assert.equal(projection.nodes.repair.state, 'failed');
  assert.equal(
    projection.terminal,
    undefined,
    'a worker error alone is not a fabricated terminal event'
  );
});

test('groups do not claim completion or failure from incomplete descendant activity', () => {
  const parallel: GraphNode = {
    kind: 'par',
    name: 'parallel',
    branches: [step('first'), step('second')],
  };
  const doc = document(
    sequence(
      'run',
      parallel,
      { kind: 'loop', name: 'rounds', body: step('review') },
      { kind: 'succeed', name: 'finished' }
    )
  );
  const history = events(
    start(ref('first')),
    complete(ref('first')),
    start(ref('review', '2', '2')),
    complete(ref('review', '2', '2'), { status: 'error', code: 'crash' }),
    success
  );
  const active = projectHistory(doc, history, 0);
  assert.equal(active.nodes.parallel.state, 'running');
  const partlyDone = projectHistory(doc, history, 1);
  assert.equal(partlyDone.nodes.first.state, 'succeeded');
  assert.equal(partlyDone.nodes.second.state, 'idle');
  assert.deepEqual(partlyDone.nodes.parallel, { state: 'recorded', count: 1 });
  assert.deepEqual(partlyDone.nodes.run, { state: 'recorded', count: 1 });
  assert.equal(
    partlyDone.nodes.finished,
    undefined,
    'terminal nodes do not have recorded dispatches'
  );
  const failedReview = projectHistory(doc, history, 3);
  assert.equal(failedReview.nodes.review.state, 'failed');
  assert.deepEqual(
    failedReview.nodes.rounds,
    { state: 'recorded', count: 1 },
    'a retry may still follow'
  );
  assert.equal(projectHistory(doc, history, 4).nodes.run.state, 'succeeded');
});

test('root failure follows its recorded terminal reason even when all workers succeeded', () => {
  const doc = document();
  const history = events(start(), complete(), {
    kind: 'terminal',
    result: { status: 'failed', reason: 'review_budget_exhausted' },
  });
  const projection = projectHistory(doc, history, 2);
  assert.equal(projection.nodes.worker.state, 'succeeded');
  assert.equal(projection.nodes.run.state, 'failed');
  assert.deepEqual(projection.terminal, { status: 'failed', reason: 'review_budget_exhausted' });
  assert.equal(projectHistory(doc, history, 1).terminal, undefined);
});

test('voiding one parallel execution neither completes peers nor conflates stopped with failed', () => {
  const doc = document(
    sequence('run', { kind: 'par', name: 'race', branches: [step('first'), step('second')] })
  );
  const history = events(start(ref('first')), start(ref('second', '2', '2')), {
    kind: 'execution_voided',
    reference: ref('second', '2', '2'),
    reason: 'parallel_join',
  });
  const projection = projectHistory(doc, history, 2);
  assert.equal(projection.nodes.first.state, 'running');
  assert.equal(projection.nodes.second.state, 'skipped');
  assert.equal(projection.nodes.race.state, 'running');
  assert.equal(projection.invocations[1].reason, 'parallel_join');
});

test('execution references preserve integers larger than Number.MAX_SAFE_INTEGER and reject rounded numbers', () => {
  const first = ref('worker', '9007199254740992', '18446744073709551614');
  const second = ref('worker', '9007199254740993', '18446744073709551615');
  const history = events(
    start(first),
    complete(first),
    start(second),
    log(second.execution as string, 'Second only')
  );
  const projection = projectHistory(document(), history, 3);
  assert.deepEqual(
    projection.invocations.map(({ id }) => id),
    ['9007199254740992', '9007199254740993']
  );
  assert.equal(projection.invocations[0].logs.length, 0);
  assert.equal(projection.invocations[1].logs[0].text, 'Second only');
  assert.throws(
    () =>
      projectHistory(
        document(),
        events(start({ ...first, execution: Number(first.execution) })),
        0
      ),
    /imprecise execution identity/
  );
});

test('mismatched node, instance or run references never settle a different invocation', () => {
  for (const mismatch of [{ node: 'other' }, { nodeInstance: '2' }, { runId: 'another-run' }]) {
    const history = events(start(), complete({ ...ref(), ...mismatch }));
    assert.equal(projectHistory(document(), history, 1).invocations[0].state, 'running');
  }
  const duplicateCompletion = events(
    start(),
    complete(),
    complete(ref(), { status: 'error', code: 'crash' })
  );
  assert.equal(
    projectHistory(document(), duplicateCompletion, 2).invocations[0].state,
    'succeeded'
  );
});

test('observed routes report only entered branches and never infer an empty terminal branch', () => {
  const choice: GraphNode = {
    kind: 'choice',
    name: 'decision',
    branches: [
      { when: {}, node: sequence('accepted', step('publish')) },
      { when: {}, node: { kind: 'fail', name: 'rejected', reason: 'rejected' } },
    ],
    otherwise: sequence('needs_work', step('repair')),
  };
  const doc = document(sequence('run', choice));
  const history = events(
    start(ref('repair')),
    complete(ref('repair')),
    start(ref('publish', '2', '2'))
  );
  assert.deepEqual(observedRoutes(choice, projectHistory(doc, history, -1).invocations), []);
  assert.deepEqual(observedRoutes(choice, projectHistory(doc, history, 1).invocations), [
    { name: 'needs_work', label: 'Otherwise', count: 1, first: 0 },
  ]);
  assert.deepEqual(
    observedRoutes(choice, projectHistory(doc, history, 2).invocations).map(({ name }) => name),
    ['needs_work', 'accepted']
  );
});

test('timeline includes trailing output without manufacturing a completion checkpoint', () => {
  const history = events(
    { kind: 'run_started' },
    start(),
    log('1', 'Work'),
    complete(),
    log('1', 'Final drained output')
  );
  const moments = historyMoments(history);
  assert.deepEqual(
    moments.map(({ index }) => index),
    [-1, 0, 1, 3, 4]
  );
  assert.equal(moments.at(-1)?.label, 'Latest recorded output');
  assert.deepEqual(historyMoments([]), [{ index: -1, cursor: 'v2:0', label: 'Before the run' }]);
});

test('a paused log cursor stays exact as new live logs and lifecycle events arrive', () => {
  const history = events(start(), log('1', 'First message'));
  const expanded = events(
    start(),
    log('1', 'First message'),
    log('1', 'Later message'),
    complete(),
    success
  );
  const before = historyMoments(history, 1).find((moment) => moment.index === 1)!;
  const after = historyMoments(expanded, 1).find((moment) => moment.index === 1)!;
  assert.equal(before.cursor, after.cursor);
  const pinned = projectHistory(document(), expanded, after.index);
  assert.equal(pinned.invocations[0].state, 'running');
  assert.deepEqual(
    pinned.invocations[0].logs.map((log) => log.text),
    ['First message']
  );
  assert.equal(pinned.terminal, undefined);
  assert.deepEqual(
    historyMoments(expanded, 3).map((moment) => moment.index),
    [-1, 0, 3, 4]
  );
});

test('node replay follows every visit and skips checkpoints with no activity in its scope', () => {
  const doc = document(
    sequence(
      'run',
      {
        kind: 'loop',
        name: 'reviews',
        body: step('review'),
      },
      step('worker'),
      step('unreached')
    )
  );
  const history = events(
    { kind: 'run_started' },
    start(ref('review', '1')),
    complete(ref('review', '1'), { status: 'verified', signals: { verdict: 'reject' } }),
    start(ref('worker', '2')),
    complete(ref('worker', '2')),
    start(ref('review', '3')),
    complete(ref('review', '3'), { status: 'verified', signals: { verdict: 'accept' } }),
    success
  );
  const moments = historyMoments(history);
  const scope = nodeReplayPositions(doc, moments, history, 'review');
  assert.deepEqual(scope, [2, 3, 6, 7]);
  assert.deepEqual(nodeReplayPositions(doc, moments, history, 'reviews'), scope);
  assert.deepEqual(nodeReplayPositions(doc, moments, history, 'run'), [0, 1, 2, 3, 4, 5, 6, 7, 8]);
  assert.deepEqual(nodeReplayPositions(doc, moments, history, 'unreached'), []);
  assert.deepEqual(nodeReplayPositions(doc, moments, history, 'missing'), []);
  assert.deepEqual(nodeReplayPositions(doc, historyMoments([]), [], 'run'), []);
  const first = projectHistory(doc, history, moments[scope[0]].index).invocations;
  assert.equal(first.length, 1);
  assert.equal(first[0].state, 'running');
  assert.equal(first[0].outcome, undefined);
  assert.equal(nextReplayPosition(0, moments.length, scope), 2);
  assert.equal(nextReplayPosition(3, moments.length, scope), 6);
  assert.equal(nextReplayPosition(4, moments.length, scope), 6);
  assert.equal(nextReplayPosition(7, moments.length, scope), undefined);
  assert.equal(nextReplayPosition(3, moments.length), 4);
  assert.equal(nextReplayPosition(8, moments.length), undefined);
  assert.equal(nextReplayPosition(0, moments.length, []), undefined);
});

test('node replay retains late output and usage without visiting unrelated later activity', () => {
  const doc = document(sequence('run', step('worker'), step('other')));
  const history = events(
    start(),
    complete(),
    log('1', 'Drained after settlement'),
    start(ref('other', '2')),
    complete(ref('other', '2')),
    { kind: 'token_usage_observed', execution: '1', usage: { inputTokens: 4 } }
  );
  const moments = historyMoments(history);
  const scope = nodeReplayPositions(doc, moments, history, 'worker');
  assert.deepEqual(scope, [1, 2, 3, 5]);
  const after = projectHistory(doc, history, moments[scope.at(-1)!].index);
  assert.equal(after.invocations[0].logs[0].text, 'Drained after settlement');
  assert.deepEqual(after.invocations[0].usage, { inputTokens: 4 });
});

test('node replay follows the active event when concurrent items finish out of start order', () => {
  const doc = document({ kind: 'map', name: 'items', body: step('worker') });
  const history = events(
    start(ref('worker', '1', '1'), 1, [0]),
    start(ref('worker', '2', '2'), 1, [1]),
    complete(ref('worker', '1', '1')),
    log('2', 'Still working'),
    { kind: 'token_usage_observed', execution: '1', usage: { inputTokens: 4 } },
    complete(ref('worker', '2', '2')),
    success
  );
  const selected = (index: number) =>
    currentInvocation(projectHistory(doc, history, index).invocations, history, index);
  assert.equal(selected(-1), undefined);
  assert.equal(selected(0)?.id, '1');
  assert.equal(selected(1)?.id, '2');
  assert.equal(selected(1)?.outcome, undefined);
  assert.equal(selected(2)?.id, '1');
  assert.equal(selected(2)?.state, 'succeeded');
  assert.equal(selected(3)?.id, '2');
  assert.equal(selected(4)?.id, '1');
  assert.equal(selected(5)?.id, '2');
  assert.equal(selected(6)?.id, '2');
});

test('system output and usage remain cursor-specific; unavailable usage stays unknown', () => {
  const history = events(
    log(null, 'Preparing'),
    start(),
    { kind: 'token_usage_observed', execution: '1', usage: { inputTokens: 10, outputTokens: 2 } },
    { kind: 'token_usage_observed', execution: '1', usage: { inputTokens: 20, outputTokens: 3 } },
    { kind: 'token_usage_observed', execution: '1', usage: null },
    { kind: 'token_usage_observed', execution: '1', usage: { inputTokens: 4, outputTokens: 5 } }
  );
  const projection = projectHistory(document(), history, 3);
  assert.equal(projection.systemLogs[0].text, 'Preparing');
  assert.deepEqual(projection.invocations[0].usage, { inputTokens: 30, outputTokens: 5 });
  assert.equal(projectHistory(document(), history, 1).invocations[0].usage, undefined);
  assert.equal(projectHistory(document(), history, 5).invocations[0].usage, null);
});

test('node names do not mutate observation object prototypes', () => {
  const doc = document(sequence('run', step('__proto__')));
  const projection = projectHistory(doc, events(start(ref('__proto__'))), 0);
  assert.equal(Object.getPrototypeOf(projection.nodes), null);
  assert.equal(Object.hasOwn(projection.nodes, '__proto__'), true);
  assert.equal(projection.nodes.__proto__.state, 'running');
});
