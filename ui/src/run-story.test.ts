import test from 'node:test';
import assert from 'node:assert/strict';
import type { Document, GraphNode } from './domain';
import { projectHistory, type HistoryEvent, type Outcome, type Reference } from './run-history';
import { formatDuration, projectOutcome, projectStory, type StoryEntry } from './run-story';

const step = (name: string): GraphNode => ({ name, kind: 'step' });
const seq = (name: string, ...children: GraphNode[]): GraphNode => ({
  name,
  kind: 'seq',
  children,
});
const par = (name: string, ...branches: GraphNode[]): GraphNode => ({
  name,
  kind: 'par',
  branches,
  join: { kind: 'all' },
});
const loop = (name: string, ...children: GraphNode[]): GraphNode => ({
  name,
  kind: 'loop',
  body: seq(`${name}_body`, ...children),
});
const map = (name: string, body: GraphNode): GraphNode => ({ name, kind: 'map', body });
function document(root: GraphNode): Document {
  return {
    name: 'story-run',
    graph: { profile: 'full-v1', initialInput: { kind: 'null' }, policy: {}, root },
    runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
  };
}
let executions = 0;
const ref = (node: string, execution = String(++executions), nodeInstance = execution) =>
  ({ node, execution, nodeInstance, runId: 'story-run' }) satisfies Reference;
const start = (reference: Reference, attempt = 1, mapIndices: number[] = []) =>
  ({
    kind: 'node_started',
    reference,
    occurrence: { node: reference.node, mapIndices },
    attempt,
  }) satisfies HistoryEvent['event'];
const complete = (reference: Reference, outcome: Outcome = { status: 'verified' }) =>
  ({ kind: 'node_completed', completion: { reference, outcome } }) satisfies HistoryEvent['event'];
const signal = (reference: Reference, key: string, value: string) =>
  complete(reference, { status: 'verifier', signals: { [key]: value } });
const say = (reference: Reference, line: string) =>
  ({
    kind: 'safe_log',
    execution: reference.execution,
    stream: 'output',
    line,
  }) satisfies HistoryEvent['event'];

function story(doc: Document, values: HistoryEvent['event'][], open = true) {
  const history = values.map((event, index) => ({ cursor: `v2:${index + 1}`, event }));
  const { invocations, terminal, handoffs } = projectHistory(doc, history, history.length - 1);
  return {
    entries: projectStory(doc, invocations, handoffs),
    outcome: projectOutcome(doc, invocations, terminal, open),
  };
}
const read = (entries: StoryEntry[]) =>
  entries.map((entry) =>
    entry.kind === 'round'
      ? `— Round ${entry.round} · ${entry.loop}`
      : entry.kind === 'continued'
        ? `↪ continued from ${entry.from}`
        : `${entry.flagged ? '!' : ' '} ${entry.steps
            .map((step) => [step.label, step.decision].filter(Boolean).join(': '))
            .join(' | ')}`
  );

test('a delivery repair reads as one chronological story with rounds, judging no signal value', () => {
  const doc = document(
    seq(
      'run',
      step('worker'),
      loop(
        'change_loop',
        par('reviews', step('acceptance'), step('code')),
        step('deliver'),
        step('delivery_repair')
      )
    )
  );
  const worker = ref('worker'),
    [acceptance1, code1, deliver1, repair] = [
      'acceptance',
      'code',
      'deliver',
      'delivery_repair',
    ].map((node) => ref(node)),
    [acceptance2, code2, deliver2] = ['acceptance', 'code', 'deliver'].map((node) => ref(node));
  const { entries, outcome } = story(doc, [
    start(worker),
    say(worker, 'Added the missing tenant boundary check.'),
    complete(worker),
    start(acceptance1),
    start(code1),
    signal(code1, 'verdict', 'accepted'),
    signal(acceptance1, 'verdict', 'accepted'),
    start(deliver1),
    say(deliver1, 'The formatting check rejected the new fixture.'),
    signal(deliver1, 'delivery', 'ci_failed'),
    start(repair),
    complete(repair),
    start(acceptance2),
    start(code2),
    signal(acceptance2, 'verdict', 'accepted'),
    signal(code2, 'verdict', 'accepted'),
    start(deliver2),
    signal(deliver2, 'delivery', 'merged'),
    { kind: 'terminal', result: { status: 'succeeded', output: null } },
  ]);
  assert.deepEqual(read(entries), [
    '  Worker',
    '— Round 1 · Change loop',
    '  Acceptance: Accepted | Code: Accepted',
    '  Deliver: Ci failed',
    '  Delivery repair',
    '— Round 2 · Change loop',
    '  Acceptance: Accepted | Code: Accepted',
    '  Deliver: Merged',
  ]);
  const reviews = entries[2];
  assert.equal(reviews.kind === 'work' && reviews.group, 'In parallel · Reviews');
  assert.equal(entries[0].kind === 'work' && entries[0].group, undefined);
  const deliver = entries[3];
  assert.equal(
    deliver.kind === 'work' && deliver.steps[0].line,
    'The formatting check rejected the new fixture.'
  );
  assert.deepEqual(outcome, {
    text: 'Run completed',
    state: 'succeeded',
    facts: ['8 steps', '2 rounds of change loop'],
  });
});

test('map items that run together share an entry, and a failed item flags it', () => {
  const doc = document(seq('run', map('suites', step('investigate')), step('report')));
  const items = [0, 1, 2].map((index) => ref('investigate', `i${index}`, `i${index}`));
  const report = ref('report');
  const { entries, outcome } = story(doc, [
    ...items.map((reference, index) => start(reference, 1, [index])),
    complete(items[0]),
    complete(items[1], { status: 'error', code: 'timeout' }),
    complete(items[2]),
    start(report),
  ]);
  assert.deepEqual(read(entries), [
    '! Investigate | Investigate: Timeout | Investigate',
    '  Report',
  ]);
  const [suites] = entries;
  assert.equal(suites.kind === 'work' && suites.group, '3 items · Suites');
  assert.deepEqual(suites.kind === 'work' && suites.steps.map((step) => step.item), [
    'Item 1',
    'Item 2',
    'Item 3',
  ]);
  assert.equal(outcome.text, 'Running Report');
});

test('a retry folds into its execution instead of appearing twice', () => {
  const doc = document(seq('run', step('build')));
  const first = ref('build', 'b1', 'b'),
    second = ref('build', 'b2', 'b');
  const { entries, outcome } = story(doc, [
    start(first),
    complete(first, { status: 'error', code: 'provider_timeout' }),
    start(second, 2),
    complete(second),
  ]);
  assert.deepEqual(read(entries), ['! Build']);
  assert.equal(entries[0].kind === 'work' && entries[0].steps[0].attempts, 2);
  assert.deepEqual(outcome.facts, ['1 step', '1 retried']);
});

test('the outcome names the round of running work, and token totals over partial usage are bounds', () => {
  const doc = document(seq('run', step('plan'), loop('rounds', step('fix'))));
  doc.runtime.nodes = { plan: { kind: 'agent' }, fix: { kind: 'agent' } };
  const plan = ref('plan'),
    fix = ref('fix');
  const { outcome } = story(doc, [
    start(plan),
    {
      kind: 'token_usage_observed',
      execution: plan.execution,
      usage: { inputTokens: 1_000, outputTokens: 500 },
    },
    complete(plan),
    start(fix),
  ]);
  assert.deepEqual(outcome, {
    text: 'Running Fix (round 1)',
    state: 'running',
    facts: ['2 steps', '1 round of rounds', '≥ 1.5K tokens'],
  });
});

test('between steps, the outcome says whether more history can still arrive', () => {
  const doc = document(seq('run', step('plan'), step('build')));
  const plan = ref('plan');
  const events: HistoryEvent['event'][] = [start(plan), complete(plan)];
  assert.equal(story(doc, events, true).outcome.text, 'Waiting for the next step');
  assert.equal(story(doc, events, false).outcome.text, 'No further history recorded');
  assert.deepEqual(story(doc, []).outcome, {
    text: 'Waiting for the next step',
    state: 'idle',
    facts: [],
  });
});

test('a question to a person reads as waiting, then as answered after a handoff to a new run', () => {
  const ask: GraphNode = { name: 'approval', kind: 'verifier', worker: 'builtin.request.ask@1' };
  const doc = document(seq('run', step('implement'), ask, step('ship')));
  const implement = ref('implement'),
    approval = ref('approval');
  const question = {
    id: 'q1',
    text: 'Approve these changes?',
    signals: { decision: ['approved', 'rejected'] },
    output: { kind: 'record', fields: {} },
    expiresAt: 9_000_000,
  };
  const paused = story(doc, [
    start(implement),
    complete(implement),
    start(approval),
    { kind: 'request_deferred', reference: approval },
    {
      kind: 'suspended',
      suspension: { reason: 'request', node: 'approval', createdAt: 1_000, request: question },
    },
  ]);
  assert.deepEqual(read(paused.entries), ['  Implement', '  Approval: Waiting for an answer']);
  const [, asking] = paused.entries;
  assert.equal(asking.kind === 'work' && asking.steps[0].request?.text, 'Approve these changes?');
  assert.deepEqual(paused.outcome, {
    text: 'Waiting for a person · Approve these changes?',
    state: 'suspended',
    facts: ['2 steps'],
  });

  const ship = ref('ship');
  const continued = story(doc, [
    {
      kind: 'prior_execution',
      execution: {
        execution: implement.execution,
        node_instance: implement.nodeInstance,
        occurrence: { node: 'implement', mapIndices: [] },
        state: { Settled: { outcome: { status: 'verified' } } },
      },
    } as unknown as HistoryEvent['event'],
    {
      kind: 'prior_execution',
      execution: {
        execution: approval.execution,
        node_instance: approval.nodeInstance,
        occurrence: { node: 'approval', mapIndices: [] },
        state: 'Active',
      },
    } as unknown as HistoryEvent['event'],
    {
      kind: 'vector',
      vector: { kind: 'request', node: 'approval', createdAt: 1_000, ...question },
    },
    { kind: 'vector', vector: { kind: 'continuation', id: 'c1', resumedFrom: 'first-run' } },
    {
      kind: 'vector',
      vector: {
        kind: 'answer',
        id: 'a1',
        createdAt: 4_000,
        requestId: 'q1',
        answer: { signals: { decision: 'approved' }, output: {} },
      },
    },
    complete(
      { ...approval, runId: 'second-run' },
      {
        status: 'verifier',
        signals: { decision: 'approved' },
      }
    ),
    start(ship),
  ]);
  assert.deepEqual(read(continued.entries), [
    '  Implement',
    '  Approval: Approved',
    '↪ continued from first-run',
    '  Ship',
  ]);
  assert.equal(continued.outcome.text, 'Running Ship');
});

test('a request that expired reads as unanswered and is flagged', () => {
  const ask: GraphNode = { name: 'approval', kind: 'verifier', worker: 'builtin.request.ask@1' };
  const doc = document(seq('run', ask));
  const approval = ref('approval');
  // The run that asked paused; its continuation carries the held request in and records
  // the expiry, then settles the request as timed out.
  const { entries } = story(doc, [
    {
      kind: 'prior_execution',
      execution: {
        execution: approval.execution,
        node_instance: approval.nodeInstance,
        occurrence: { node: 'approval', mapIndices: [] },
        attempt: 1,
        input: { text: 'Approve?' },
        state: 'Active',
      },
    } as unknown as HistoryEvent['event'],
    {
      kind: 'vector',
      vector: {
        kind: 'request',
        id: 'q1',
        node: 'approval',
        text: 'Approve?',
        signals: { decision: ['approved'] },
        output: null,
        expiresAt: 5,
      },
    },
    { kind: 'vector', vector: { kind: 'expiry', id: 'e1', requestId: 'q1', createdAt: 6 } },
    complete(approval, { status: 'error', code: 'timeout' }),
  ]);
  assert.deepEqual(read(entries), ['! Approval: No answer in time']);
});

test('a step that first runs in a later round is filed under that round, not under its own count', () => {
  const doc = document(seq('run', loop('rounds', step('review'), step('deliver'))));
  const [review1, review2, deliver] = ['review', 'review', 'deliver'].map((node) => ref(node));
  const { entries } = story(doc, [
    start(review1),
    signal(review1, 'verdict', 'rejected'),
    start(review2),
    signal(review2, 'verdict', 'accepted'),
    start(deliver),
    complete(deliver),
  ]);
  assert.deepEqual(read(entries), [
    '— Round 1 · Rounds',
    '  Review: Rejected',
    '— Round 2 · Rounds',
    '  Review: Accepted',
    '  Deliver',
  ]);
});

test('durations come from recorded times; running steps count to the position, carried-over ones stay unknown', () => {
  const doc = document(seq('run', step('plan'), step('build'), step('ship')));
  const [plan, buildA, buildB, ship] = [
    ref('plan'),
    ref('build', 'b1', 'b'),
    ref('build', 'b2', 'b'),
    ref('ship'),
  ];
  const timed = (event: HistoryEvent['event'], recordedAt: number) => ({ event, recordedAt });
  const history = [
    timed(
      {
        kind: 'prior_execution',
        execution: {
          execution: plan.execution,
          node_instance: plan.nodeInstance,
          occurrence: { node: 'plan', mapIndices: [] },
          state: { Settled: { outcome: { status: 'verified' } } },
        },
      } as unknown as HistoryEvent['event'],
      1_000
    ),
    timed(start(buildA), 2_000),
    timed(complete(buildA, { status: 'error', code: 'provider_timeout' }), 5_000),
    timed(start(buildB, 2), 6_000),
    timed(complete(buildB), 74_000),
    timed(start(ship), 80_000),
    timed(
      { kind: 'safe_log', execution: ship.execution, stream: 'output', line: 'Pushing' },
      95_000
    ),
  ].map((entry, index) => ({ cursor: `v2:${index + 1}`, ...entry }));
  const { invocations, handoffs, clock, terminal } = projectHistory(
    doc,
    history,
    history.length - 1
  );
  const steps = projectStory(doc, invocations, handoffs, clock.at).flatMap((entry) =>
    entry.kind === 'work' ? entry.steps : []
  );
  assert.deepEqual(
    steps.map((item) => [
      item.label,
      item.durationMs === undefined ? '—' : formatDuration(item.durationMs),
    ]),
    [
      ['Plan', '—'],
      ['Build', '1m 12s'],
      ['Ship', '15s'],
    ]
  );
  const outcome = projectOutcome(doc, invocations, terminal, true, clock);
  assert.ok(outcome.facts.includes('1m 34s so far'), outcome.facts.join(' | '));
});

test('a step that uses no model never makes the token total a lower bound', () => {
  const doc = document(seq('run', step('write'), step('deliver')));
  doc.runtime.nodes = { write: { kind: 'agent' }, deliver: { kind: 'git_delivery' } };
  const write = ref('write'),
    deliver = ref('deliver');
  const { outcome } = story(doc, [
    start(write),
    {
      kind: 'token_usage_observed',
      execution: write.execution,
      usage: { inputTokens: 2_000, outputTokens: 500 },
    },
    complete(write),
    start(deliver),
    complete(deliver),
  ]);
  assert.ok(outcome.facts.includes('2.5K tokens'), outcome.facts.join(' | '));
});
