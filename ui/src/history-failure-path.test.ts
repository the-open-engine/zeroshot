import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ControlRecord, ControlVisit } from './control-history';
import type { Document, GraphNode } from './domain';
import { historyMoments, projectHistory, type HistoryEvent, type Outcome } from './run-history';
import { createViteTestServer } from './test-support';

// The built-in software-change graph with `--delivery none` (native_v2_templates.rs), reduced to
// the structure the history projection reads. Guards are not evaluated by the UI.
const fail = (name: string, reason = name): GraphNode => ({ kind: 'fail', name, reason });
const verifier = (name: string): GraphNode => ({ kind: 'verifier', name, attempts: 2 });
const softwareChange: Document = {
  name: 'software-change',
  graph: {
    profile: 'openengine.graph.full/v1',
    initialInput: {
      kind: 'record',
      fields: { task: { type: { kind: 'string' }, required: true } },
    },
    policy: {},
    root: {
      kind: 'seq',
      name: 'run',
      children: [
        {
          kind: 'loop',
          name: 'change_loop',
          maxIterations: 10,
          body: {
            kind: 'seq',
            name: 'change_iteration',
            children: [
              { kind: 'step', name: 'worker', attempts: 1 },
              {
                kind: 'choice',
                name: 'worker_result',
                branches: [{ when: {}, node: fail('worker_failed') }],
                otherwise: {
                  kind: 'seq',
                  name: 'reviews',
                  children: [
                    {
                      kind: 'par',
                      name: 'parallel_reviews',
                      join: { kind: 'all' },
                      branches: [verifier('acceptance'), verifier('code')],
                    },
                    {
                      kind: 'choice',
                      name: 'review_result',
                      branches: [
                        { when: {}, node: fail('review_failed') },
                        {
                          when: {},
                          node: { kind: 'succeed', name: 'done', output: { kind: 'null' } },
                        },
                      ],
                      otherwise: {
                        kind: 'map',
                        name: 'review_retry',
                        body: fail('review_retry_invalid', 'invalid_retry_state'),
                      },
                    },
                  ],
                },
              },
            ],
          },
        },
        fail('change_attempts_exhausted'),
      ],
    },
  },
  runtime: { harness: 'claude', provider: 'anthropic', size: 'small', nodes: {} },
};
// Semantic groups the collapsed canvas draws on the path from the root to review_result.
const pathGroups = ['change_loop', 'worker_result', 'parallel_reviews', 'review_result'];

const reference = (node: string, execution: number, instance: number) => ({
  node,
  execution: String(execution),
  nodeInstance: String(instance),
});
const started = (node: string, execution: number, instance: number, attempt = 1) => ({
  kind: 'node_started',
  reference: reference(node, execution, instance),
  occurrence: { node, mapIndices: [] },
  attempt,
  input: { task: 'say hi' },
});
const completed = (node: string, execution: number, instance: number, outcome: Outcome) => ({
  kind: 'node_completed',
  completion: { reference: reference(node, execution, instance), outcome },
});
const crash: Outcome = { status: 'error', code: 'crash', reason: 'declared_failure' };
const accepted: Outcome = {
  status: 'verifier',
  output: null,
  signals: { verdict: 'accepted' },
  diagnostic: { message: 'Checked.' },
};
const ledger = (events: HistoryEvent['event'][]): HistoryEvent[] =>
  events.map((event, index) => ({ cursor: `v2:${index + 1}`, event }));

// Round 1: the worker succeeds, both verifiers crash on both attempts, review_result selects
// review_failed and the run ends failed in that round.
const failedEvents = ledger([
  { kind: 'run_started' },
  started('worker', 1, 1),
  completed('worker', 1, 1, { status: 'verified', output: null }),
  started('acceptance', 2, 2),
  started('code', 3, 3),
  completed('acceptance', 2, 2, crash),
  completed('code', 3, 3, crash),
  started('acceptance', 4, 2, 2),
  started('code', 5, 3, 2),
  completed('acceptance', 4, 2, crash),
  completed('code', 5, 3, crash),
  { kind: 'terminal', result: { status: 'failed', reason: 'review_failed' } },
]);
const succeededEvents = ledger([
  ...failedEvents.slice(0, 5).map(({ event }) => event),
  completed('acceptance', 2, 2, accepted),
  completed('code', 3, 3, accepted),
  { kind: 'terminal', result: { status: 'succeeded', output: null } },
]);

// Shaped like ControlRecord::from_trace: visitId is [node, mapIndices, loopIterations], groups are
// entered where the reducer first reaches them, and every visit the settling reduction ends is
// 'completed' unless it is the succeed or fail node itself.
function control(settledAt: string, route: 'review_failed' | 'done'): ControlRecord[] {
  const record = (
    cursor: string,
    node: string,
    loop: number[],
    state: ControlRecord['state'],
    extra: Partial<ControlRecord> = {}
  ): ControlRecord => ({
    cursor,
    node,
    mapIndices: [],
    visitId: JSON.stringify([node, [], loop]),
    state,
    ...extra,
  });
  return [
    record('v2:1', 'run', [], 'entered'),
    record('v2:1', 'change_loop', [], 'entered'),
    record('v2:1', 'change_iteration', [1], 'entered'),
    record('v2:3', 'worker_result', [1], 'entered', { branch: 'reviews' }),
    record('v2:3', 'reviews', [1], 'entered'),
    record('v2:3', 'parallel_reviews', [1], 'entered'),
    record(settledAt, 'run', [], 'completed'),
    record(settledAt, 'change_loop', [], 'completed'),
    record(settledAt, 'change_iteration', [1], 'completed'),
    record(settledAt, 'worker_result', [1], 'completed', { branch: 'reviews' }),
    record(settledAt, 'reviews', [1], 'completed'),
    record(settledAt, 'parallel_reviews', [1], 'completed', { detail: 'reached' }),
    record(settledAt, 'review_result', [1], 'completed', { branch: route }),
    route === 'review_failed'
      ? record(settledAt, 'review_failed', [1], 'failed', { detail: 'review_failed' })
      : record(settledAt, 'done', [1], 'succeeded', { output: null }),
  ];
}
const failedControl = control('v2:11', 'review_failed');
const succeededControl = control('v2:7', 'done');

async function controlTimeline(t: TestContext) {
  const server = await createViteTestServer(t);
  const { ControlTimeline } = await server.ssrLoadModule('/src/ControlTimeline.tsx');
  // RunHistoryView passes the projected visits of the selected group.
  return (visits: ControlVisit[], node: string) =>
    renderToStaticMarkup(
      createElement(ControlTimeline, {
        visits: visits.filter((visit) => visit.node === node),
        select() {},
        reveal() {},
        playing: false,
        canReplay: false,
        play() {},
        restart() {},
      })
    );
}

test('groups on the path to a fail node are never drawn as succeeded', async (t) => {
  const final = projectHistory(
    softwareChange,
    failedEvents,
    failedEvents.length - 1,
    failedControl
  );
  assert.equal(final.terminal?.status, 'failed');
  assert.equal(final.nodes.run?.state, 'failed');
  assert.equal(final.nodes.review_failed?.state, 'failed');
  for (const name of pathGroups) {
    const state = final.nodes[name]?.state;
    assert.ok(state, `${name} keeps an observation after the run ended`);
    assert.notEqual(state, 'succeeded', `${name} ended on the failure path but is drawn succeeded`);
    assert.ok(
      !['running', 'idle', 'skipped'].includes(state),
      `${name} ended on the failure path but is drawn ${state}`
    );
  }
  for (const moment of historyMoments(failedEvents)) {
    const view = projectHistory(softwareChange, failedEvents, moment.index, failedControl);
    for (const name of pathGroups)
      assert.notEqual(
        view.nodes[name]?.state,
        'succeeded',
        `${name} is drawn succeeded at ${moment.cursor} of a run that failed`
      );
  }
  const timeline = await controlTimeline(t);
  for (const name of pathGroups)
    assert.doesNotMatch(
      timeline(final.controls, name),
      /state-succeeded/,
      `the ${name} control timeline lists a visit on the failure path as succeeded`
    );
});

test('groups of a run that succeeds through its finish node are not drawn as failed', async (t) => {
  const final = projectHistory(
    softwareChange,
    succeededEvents,
    succeededEvents.length - 1,
    succeededControl
  );
  assert.equal(final.nodes.run?.state, 'succeeded');
  assert.equal(final.nodes.done?.state, 'succeeded');
  for (const name of pathGroups) {
    const state = final.nodes[name]?.state;
    assert.ok(state, `${name} keeps an observation after the run ended`);
    assert.ok(!['failed', 'running'].includes(state), `${name} is drawn ${state}`);
  }
  const timeline = await controlTimeline(t);
  for (const name of pathGroups)
    assert.doesNotMatch(timeline(final.controls, name), /state-failed/, `${name} timeline`);
});
