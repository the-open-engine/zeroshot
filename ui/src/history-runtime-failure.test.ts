import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError, createApiClient } from './api';
import {
  finishRunHistory,
  projectHistory,
  type HistoryEvent,
  type HistoryPage,
  type RunDetail,
} from './run-history';
import { appendHistory, createRunHistorySource, readRunDetail } from './run-history-source';

const running = (): RunDetail => ({
  version: 1,
  projectionVersion: 1,
  runId: 'retained-run',
  title: 'Review the implementation',
  phase: 'running',
  cursor: 'v2:3',
  historyAvailable: true,
  graph: {
    profile: 'openengine.graph.full/v1',
    initialInput: { kind: 'null' },
    policy: {},
    root: { kind: 'seq', name: 'run', children: [{ kind: 'step', name: 'worker' }] },
  },
  runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
  initialInput: null,
  snapshot: { phase: 'running', cursor: 'v2:3', terminal: null },
  history: { initialCursor: 'v2:0', cursor: 'v2:3', complete: false, limitations: [] },
});
const events = (): HistoryEvent[] => [
  { cursor: 'v2:1', event: { kind: 'run_started' } },
  {
    cursor: 'v2:2',
    event: {
      kind: 'node_started',
      reference: { node: 'worker', execution: '1', nodeInstance: '1' },
      occurrence: { node: 'worker', mapIndices: [] },
      input: null,
    },
  },
  { cursor: 'v2:3', event: { kind: 'safe_log', execution: '1', line: 'Inspecting the change.' } },
];
const failure = { atCursor: 'v2:2', reason: 'runtime_failed' } as const;
const failedPage = (): HistoryPage => ({
  events: [],
  nextCursor: 'v2:3',
  headCursor: 'v2:3',
  complete: true,
  finished: true,
  runtimeFailure: failure,
});
const invalidFailure = (error: unknown) =>
  error instanceof ApiError && error.code === 'invalid_runtime_failure';

test('confirmed runtime failure settles current status without changing the replay or snapshot', () => {
  const detail = running();
  const retained = events();
  const before = structuredClone({ detail, retained });
  const collected = appendHistory(retained, failedPage());
  const finished = finishRunHistory(detail, collected, failedPage());

  assert.equal(finished.phase, 'finished');
  assert.deepEqual(finished.terminal, { status: 'failed', reason: 'runtime_failed' });
  assert.deepEqual(finished.runtimeFailure, failure);
  assert.equal(finished.history.complete, false);
  assert.deepEqual(finished.snapshot, detail.snapshot);
  assert.deepEqual(collected, retained);
  assert.deepEqual({ detail, retained }, before);
  assert.equal(readRunDetail(finished, detail.runId), finished);

  const document = { name: detail.runId, graph: detail.graph, runtime: detail.runtime };
  const latest = projectHistory(document, collected, 2);
  assert.equal(latest.terminal, undefined);
  assert.equal(latest.nodes.worker.state, 'running');
  assert.equal(latest.invocations[0].outcome, undefined);
  assert.equal(latest.invocations[0].end, undefined);
  const rewind = projectHistory(document, collected, 0);
  assert.equal(rewind.nodes.worker.state, 'idle');
  assert.equal(rewind.invocations.length, 0);
  assert.equal(rewind.terminal, undefined);
});

test('a current failure reason survives a drained page without a terminal event', () => {
  const failed = finishRunHistory(running(), events(), failedPage());
  const reloaded = finishRunHistory(failed, events(), { headCursor: 'v2:3' });
  assert.deepEqual(reloaded.terminal, failed.terminal);
  assert.deepEqual(reloaded.runtimeFailure, failure);
  assert.equal(reloaded.history.complete, false);
});

test('later durable settlement supersedes the fallback and makes history complete', () => {
  const failed = finishRunHistory(running(), events(), failedPage());
  const terminal = { status: 'failed', reason: 'recorded_failure' } as const;
  const page: HistoryPage = {
    events: [{ cursor: 'v2:4', event: { kind: 'terminal', result: terminal } }],
    nextCursor: 'v2:4',
    headCursor: 'v2:4',
    complete: true,
    finished: true,
  };
  const collected = appendHistory(events(), page);
  const recovered = finishRunHistory(failed, collected, page);
  assert.deepEqual(recovered.terminal, terminal);
  assert.equal(recovered.runtimeFailure, undefined);
  assert.equal(recovered.history.complete, true);
  assert.equal(recovered.history.cursor, 'v2:4');
  assert.equal(recovered.cursor, 'v2:4');
  const document = { name: failed.runId, graph: failed.graph, runtime: failed.runtime };
  assert.deepEqual(projectHistory(document, collected, 3).terminal, terminal);
  assert.equal(projectHistory(document, collected, 2).terminal, undefined);
});

test('failure anchors use canonical bounded cursors at or before the durable head', () => {
  const detail = finishRunHistory(running(), events(), failedPage());
  for (const atCursor of ['v2:0', 'v2:2', 'v2:3']) {
    const value = { ...detail, runtimeFailure: { ...failure, atCursor } };
    assert.equal(readRunDetail(value, detail.runId), value);
  }
  for (const runtimeFailure of [
    null,
    {},
    { ...failure, reason: 'unknown_failure' },
    ...['v2:4', 'v2:-1', 'v2:01', 'v2:1.0', 'v2:9223372036854775808', 'other:2', null].map(
      (atCursor) => ({ ...failure, atCursor })
    ),
  ]) {
    assert.throws(() => readRunDetail({ ...detail, runtimeFailure }, detail.runId), invalidFailure);
    assert.throws(
      () => appendHistory(events(), { ...failedPage(), runtimeFailure } as HistoryPage),
      invalidFailure
    );
  }
  for (const override of [
    { phase: 'running' },
    { terminal: null },
    { terminal: { status: 'succeeded', output: null } },
    { terminal: { status: 'failed', reason: 'other' } },
    { history: { ...detail.history, complete: true } },
    { cursor: 'v2:2' },
  ])
    assert.throws(() => readRunDetail({ ...detail, ...override }, detail.runId), invalidFailure);

  // A confirmed failure can precede draining the retained pages. Its anchor is not a replay event.
  const partial = {
    ...failedPage(),
    events: events().slice(0, 1),
    nextCursor: 'v2:1',
    complete: false,
  };
  assert.deepEqual(appendHistory([], partial), events().slice(0, 1));
});

test('HTTP pages reject a failure marker that does not describe a finished runtime', async () => {
  const base = new URL('https://example.test/ui/api/');
  const source = createRunHistorySource(
    createApiClient(base, async () => Response.json({ ...failedPage(), finished: false })),
    base
  );
  await assert.rejects(source.page('retained-run', 'v2:3'), invalidFailure);
});
