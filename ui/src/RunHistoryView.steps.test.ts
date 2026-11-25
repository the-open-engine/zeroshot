import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement } from 'react';
import type { HistoryPage, RunDetail } from './run-history';
import type { RunHistoryReader } from './run-history-source';
import { runDetailFixture } from './test-support';
import {
  stubWorker,
  viewerModule,
  completePage,
  renderedText,
  clickButton,
  showGraph,
  installBrowser,
  flushEffects,
} from './run-history-test-support';

test('opening steps keeps a live run followed; Show node and a node picked in Graph open it in Steps', async (t) => {
  stubWorker(t);
  installBrowser(t);
  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const base = runDetailFixture('live-run');
  const detail: RunDetail = {
    ...base,
    graph: {
      ...base.graph,
      root: {
        kind: 'seq',
        name: 'run',
        children: [
          { kind: 'step', name: 'plan' },
          { kind: 'step', name: 'build' },
        ],
      },
    },
    observation: { state: 'active' },
  };
  const reference = (node: string, execution: string) => ({
    node,
    execution,
    nodeInstance: execution,
    runId: detail.runId,
  });
  const events: HistoryPage['events'] = [
    { cursor: 'v2:1', event: { kind: 'run_started' } },
    { cursor: 'v2:2', event: { kind: 'node_started', reference: reference('plan', '1') } },
    {
      cursor: 'v2:3',
      event: { kind: 'safe_log', execution: '1', stream: 'output', line: 'Planning the change' },
    },
  ];
  let observer: Parameters<NonNullable<RunHistoryReader['watch']>>[2] | undefined;
  const source: RunHistoryReader = {
    detail: async () => detail,
    page: async () => ({ ...completePage(), events, nextCursor: 'v2:3', headCursor: 'v2:3' }),
    watch(_id, _after, next) {
      observer = next;
      return () => {};
    },
  };
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  const following = () => container.querySelector('.history-live')?.getAttribute('title');
  try {
    await act(async () =>
      root.render(createElement(RunHistoryView, { runId: detail.runId, source, workers: [] }))
    );
    await flushEffects();
    assert.ok(observer);
    await act(async () => observer!.status('connected'));
    assert.equal(following(), 'Following latest events');
    const plan = [...container.querySelectorAll<HTMLButtonElement>('.story-step-head')].find(
      (head) => head.textContent?.includes('Plan')
    );
    assert.ok(plan);
    await act(async () => plan.click());
    assert.match(container.querySelector('.story-step.open')!.textContent!, /Planning the change/);
    assert.equal(following(), 'Following latest events', 'opening a step is not a seek');

    await act(async () =>
      observer!.page({
        ...completePage(),
        events: [
          { cursor: 'v2:4', event: { kind: 'node_started', reference: reference('build', '2') } },
        ],
        nextCursor: 'v2:4',
        headCursor: 'v2:4',
      })
    );
    assert.match(
      container.querySelector('.timeline-caption strong')!.textContent!,
      /Build/,
      'the live run is still followed after a step was opened'
    );

    // A node picked in Graph is still picked after switching back to Steps.
    await showGraph(container);
    const graphShowNode = [...container.querySelectorAll<HTMLButtonElement>('button')].find(
      (button) => button.textContent === 'Show node'
    );
    assert.ok(graphShowNode);
    await act(async () => graphShowNode.click());
    const steps = [...container.querySelectorAll<HTMLButtonElement>('.view-toggle button')].find(
      (button) => button.textContent === 'Steps'
    );
    assert.ok(steps);
    await act(async () => steps.click());
    const reopened = [...container.querySelectorAll('.story-step.open')].map(
      (row) => row.textContent
    );
    assert.ok(
      reopened.some((text) => text?.includes('Build')),
      reopened.join(' | ')
    );

    const showNode = [...container.querySelectorAll<HTMLButtonElement>('button')].find(
      (button) => button.textContent === 'Show node'
    );
    assert.ok(showNode);
    await act(async () => showNode.click());
    const open = [...container.querySelectorAll('.story-step.open')].map((row) => row.textContent);
    assert.ok(
      open.some((text) => text?.includes('Build')),
      open.join(' | ')
    );
    assert.ok(container.querySelector('.run-steps-view'), 'Show node stays in Steps');
    assert.equal(following(), 'Following latest events');
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});

/** A complete recorded pause, including the request vector before the final suspension. */
async function withSuspendedSteps(
  t: TestContext,
  reason: 'request' | 'wait' | 'command',
  check: (view: {
    container: HTMLElement;
    render: () => Promise<void>;
    reads: () => number[];
    expiresAt: number;
  }) => Promise<void>,
  knownSuccessor?: string
) {
  stubWorker(t);
  installBrowser(t);
  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const expiresAt = Date.parse('2026-12-01T08:00:00Z');
  t.mock.method(Date, 'now', () => expiresAt - 60_000);
  const request = {
    id: 'question',
    text: 'Approve the release?',
    signals: { decision: ['approved', 'rejected'] },
    output: null,
    expiresAt,
  };
  const suspension: NonNullable<RunDetail['suspension']> = {
    reason,
    node: reason === 'request' ? 'approval' : 'cooldown',
    createdAt: expiresAt - 60_000,
    ...(reason === 'request'
      ? { request }
      : reason === 'wait'
        ? { wait: { dueAt: expiresAt } }
        : {}),
  };
  const detail: RunDetail = {
    ...runDetailFixture('paused-run'),
    phase: 'suspended',
    suspension,
    ...(knownSuccessor && {
      partId: 'paused-run',
      currentPartId: knownSuccessor,
      parts: ['paused-run', knownSuccessor],
      partView: true,
    }),
    observation: { state: 'complete' },
    graph: {
      ...runDetailFixture().graph,
      root: {
        kind: 'par',
        name: 'run',
        join: { kind: 'all' },
        branches: [
          { kind: 'step', name: 'held' },
          {
            kind: 'verifier',
            name: suspension.node,
            worker: reason === 'request' ? 'builtin.request.ask@1' : 'builtin.wait.until@1',
          },
          { kind: 'step', name: 'queued' },
        ],
      },
    },
  };
  const reference = (node: string) => ({ node, execution: node, nodeInstance: node });
  const values: HistoryPage['events'][number]['event'][] = [
    { kind: 'run_started' },
    { kind: 'node_started', reference: reference('held') },
    { kind: 'node_started', reference: reference(suspension.node) },
    ...(reason === 'command'
      ? []
      : [
          {
            kind: reason === 'request' ? 'request_deferred' : 'wait_deferred',
            reference: reference(suspension.node),
          },
        ]),
    ...(reason === 'request'
      ? [
          {
            kind: 'vector',
            vector: {
              kind: 'request',
              ...request,
              node: suspension.node,
              createdAt: suspension.createdAt,
            },
          },
        ]
      : []),
    { kind: 'suspended', suspension },
  ];
  const events = values.map((event, index) => ({
    cursor: `v2:${index + 1}`,
    recordedAt: expiresAt - 64_000 + index * 1_000,
    event,
  }));
  let detailReads = 0,
    pageReads = 0;
  const source: RunHistoryReader = {
    detail: async () => {
      detailReads++;
      return detail;
    },
    page: async () => {
      pageReads++;
      return {
        ...completePage(),
        events,
        nextCursor: events.at(-1)!.cursor,
        headCursor: events.at(-1)!.cursor,
        finished: true,
        control: [
          { cursor: 'v2:1', node: 'run', visitId: 'run', mapIndices: [], state: 'entered' },
          { cursor: 'v2:1', node: 'queued', visitId: 'queued', mapIndices: [], state: 'queued' },
        ],
      };
    },
  };
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  const render = async () => {
    await act(async () =>
      root.render(
        createElement(RunHistoryView, {
          runId: detail.runId,
          source,
          workers: [],
          partHref: (id: string) => `/host/runs/paused-run/parts/${id}`,
        })
      )
    );
    await flushEffects();
  };
  try {
    await check({ container, render, reads: () => [detailReads, pageReads], expiresAt });
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
}

const approvalRow = (container: HTMLElement) =>
  container.querySelector<HTMLElement>('[data-invocation="approval"]')!;

test('opening a step during playback keeps the shared replay timer running', async (t) => {
  await withSuspendedSteps(t, 'request', async ({ container, render, expiresAt }) => {
    t.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: expiresAt - 60_000 });
    await render();
    await clickButton(container, 'Go to start');
    await clickButton(container, 'Next event');
    await clickButton(container, 'Next event');
    await clickButton(container, 'Play replay');
    const row = container.querySelector<HTMLElement>('[data-invocation="held"]')!;
    await act(async () => row.querySelector('button')!.click());
    assert.equal(row.querySelector('button')?.getAttribute('aria-expanded'), 'true');
    assert.ok(container.querySelector('[aria-label="Pause replay"]'));
    const caption = container.querySelector('.timeline-caption')!.textContent;
    await act(async () => t.mock.timers.tick(950));
    assert.notEqual(container.querySelector('.timeline-caption')!.textContent, caption);
    assert.ok(container.querySelector('[aria-label="Pause replay"]'));
  });
});

test('mounted Steps expires an open question at its deadline without refreshing history', async (t) => {
  await withSuspendedSteps(t, 'request', async ({ container, render, reads, expiresAt }) => {
    t.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: expiresAt - 60_000 });
    await render();
    await act(async () => approvalRow(container).querySelector('button')!.click());
    assert.match(approvalRow(container).textContent!, /Answer with/);
    await act(async () => t.mock.timers.tick(60_000));
    assert.match(approvalRow(container).textContent!, /Expired|Deadline passed|deadline passed/);
    assert.doesNotMatch(
      approvalRow(container).textContent!,
      /Waiting for an answer|Answer with|No answer before/
    );
    assert.match(
      container.querySelector('.run-outcome')!.textContent!,
      /[Dd]eadline passed|[Ee]xpired/
    );
    assert.deepEqual(reads(), [1, 1]);
    await act(async () => approvalRow(container).querySelector('button')!.click());
    assert.match(approvalRow(container).textContent!, /Deadline passed/);
    assert.doesNotMatch(
      approvalRow(container).textContent!,
      /Waiting for an answer|No answer before/
    );
    await clickButton(container, 'Previous event');
    assert.match(approvalRow(container).textContent!, /Waiting for an answer/);
    assert.doesNotMatch(approvalRow(container).textContent!, /deadline passed/);
  });
});

test('a predecessor opened with a known successor replays its recorded question as waiting', async (t) => {
  await withSuspendedSteps(
    t,
    'request',
    async ({ container, render }) => {
      await render();
      assert.match(approvalRow(container).textContent!, /Handed off/);
      assert.match(
        container.querySelector('.run-outcome')!.textContent!,
        /Continued in part continued-run/
      );
      assert.equal(
        container.querySelector('.run-handoff')?.getAttribute('href'),
        '/host/runs/paused-run/parts/continued-run'
      );
      await clickButton(container, 'Previous event');
      assert.match(approvalRow(container).textContent!, /Waiting for an answer/);
      assert.doesNotMatch(approvalRow(container).textContent!, /closed without|Handed off/);
      await act(async () => approvalRow(container).querySelector('button')!.click());
      assert.match(approvalRow(container).textContent!, /Waiting since/);
      assert.doesNotMatch(
        approvalRow(container).textContent!,
        /closed without|Handed off|Answer with/
      );
      assert.equal(container.querySelector('.run-handoff'), null);
    },
    'continued-run'
  );
});

for (const reason of ['wait', 'command'] as const) {
  test(`mounted Steps shows a ${reason} suspension and held work at the selected position`, async (t) => {
    await withSuspendedSteps(t, reason, async ({ container, render }) => {
      await render();
      const outcome = container.querySelector('.run-outcome')!;
      assert.match(outcome.textContent!, /Suspended/);
      if (reason === 'wait') assert.match(outcome.textContent!, /2026-12-01T08:00:00.000Z/);
      assert.doesNotMatch(outcome.textContent!, /No further history recorded|so far|Running/);
      assert.equal(container.querySelector('.story-step.state-running'), null);
      assert.ok(container.querySelector('[data-invocation="held"].state-suspended'));
      const duration = outcome.querySelector('.run-outcome-facts')!.textContent!;
      await clickButton(container, 'Previous event');
      assert.match(container.querySelector('.run-outcome')!.textContent!, /Running/);
      assert.ok(container.querySelector('[data-invocation="held"].state-running'));
      assert.notEqual(container.querySelector('.run-outcome-facts')!.textContent!, duration);
      await showGraph(container);
      await clickButton(container, 'Go to latest');
      assert.doesNotMatch(renderedText(container), /1 queued/);
    });
  });
}

test('a force-stopped deferred timer shows stopped in Steps only after its recorded terminal', async (t) => {
  stubWorker(t);
  installBrowser(t);
  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const terminal = { status: 'failed' as const, reason: 'force_stopped' };
  const suspension = {
    reason: 'wait' as const,
    node: 'cooldown',
    createdAt: 4000,
    wait: { dueAt: 9000 },
  };
  const base = runDetailFixture('stopped-timer');
  const detail: RunDetail = {
    ...base,
    phase: 'finished',
    terminal,
    observation: { state: 'complete' },
    graph: {
      ...base.graph,
      root: {
        kind: 'seq',
        name: 'run',
        children: [{ kind: 'verifier', name: 'cooldown', worker: 'builtin.wait.until@1' }],
      },
    },
  };
  const reference = { node: 'cooldown', execution: '43', nodeInstance: '17' };
  // Native force-stop leaves the deferred timer without a synthetic node completion.
  const values: HistoryPage['events'][number]['event'][] = [
    { kind: 'run_started' },
    { kind: 'node_started', reference },
    { kind: 'wait_deferred', reference },
    { kind: 'suspended', suspension },
    { kind: 'force_stop_requested' },
    { kind: 'terminal', result: terminal },
  ];
  const events = values.map((event, index) => ({
    cursor: `v2:${index + 1}`,
    recordedAt: (index + 1) * 1000,
    event,
  }));
  const source: RunHistoryReader = {
    detail: async () => detail,
    page: async () => ({
      ...completePage(),
      events,
      nextCursor: 'v2:6',
      headCursor: 'v2:6',
      finished: true,
    }),
  };
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  const row = () => container.querySelector<HTMLElement>('[data-invocation="43"]')!;
  const assertStopped = () => {
    assert.match(
      container.querySelector('.run-outcome')!.textContent!,
      /Run failed · Force stopped/
    );
    assert.ok(row().classList.contains('state-skipped'));
    assert.match(row().querySelector('.story-decision')!.textContent!, /Stopped/);
    assert.doesNotMatch(row().textContent!, /Waiting|In progress/);
    assert.equal(row().querySelector('.story-time')!.textContent, '1s');
  };
  try {
    await act(async () =>
      root.render(createElement(RunHistoryView, { runId: detail.runId, source, workers: [] }))
    );
    await flushEffects();
    assertStopped();
    await act(async () => row().querySelector('button')!.click());
    assertStopped();
    await clickButton(container, 'Previous event');
    assert.ok(row().classList.contains('state-suspended'));
    assert.match(row().querySelector('.story-decision')!.textContent!, /Waiting/);
    assert.match(
      container.querySelector('.run-outcome')!.textContent!,
      /Suspended \(wait\).*1970-01-01T00:00:09.000Z/
    );
    await clickButton(container, 'Previous event');
    assert.ok(row().classList.contains('state-suspended'));
    assert.match(row().querySelector('.story-decision')!.textContent!, /Waiting/);
    await clickButton(container, 'Go to latest');
    assertStopped();
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});

/** Sparse execution IDs are opaque; retries share a logical occurrence, not an ID sequence. */
async function withRetriedSteps(
  t: TestContext,
  check: (view: {
    container: HTMLElement;
    retry: () => Promise<void>;
    finish: () => Promise<void>;
  }) => Promise<void>,
  live = false
) {
  stubWorker(t);
  installBrowser(t);
  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const base = runDetailFixture('retry-run');
  const detail: RunDetail = {
    ...base,
    graph: {
      ...base.graph,
      root: { kind: 'seq', name: 'run', children: [{ kind: 'verifier', name: 'review' }] },
    },
    observation: { state: live ? 'active' : 'complete' },
  };
  const reference = (execution: string) => ({
    node: 'review',
    execution,
    nodeInstance: 'review-instance',
    runId: detail.runId,
  });
  const first = reference('attempt-17'),
    second = reference('attempt-91');
  const values: HistoryPage['events'][number]['event'][] = [
    { kind: 'run_started' },
    { kind: 'node_started', reference: first, attempt: 1, input: { prompt: 'First input' } },
    { kind: 'safe_log', execution: first.execution, stream: 'output', line: 'First transcript' },
    {
      kind: 'node_completed',
      completion: {
        reference: first,
        outcome: {
          status: 'error',
          code: 'provider_timeout',
          output: { result: 'First result' },
        },
      },
    },
    { kind: 'node_started', reference: second, attempt: 2, input: { prompt: 'Second input' } },
    { kind: 'safe_log', execution: second.execution, stream: 'output', line: 'Second transcript' },
    {
      kind: 'node_completed',
      completion: {
        reference: second,
        outcome: {
          status: 'verifier',
          signals: { code_review: 'accepted', test_review: 'rejected' },
          output: { result: 'Second result' },
        },
      },
    },
  ];
  const events = values.map((event, index) => ({ cursor: `v2:${index + 1}`, event }));
  const page = (items: typeof events): HistoryPage => ({
    ...completePage(),
    events: items,
    nextCursor: items.at(-1)!.cursor,
    headCursor: items.at(-1)!.cursor,
  });
  let observer: Parameters<NonNullable<RunHistoryReader['watch']>>[2] | undefined;
  const source: RunHistoryReader = {
    detail: async () => detail,
    page: async () => page(live ? events.slice(0, 4) : events),
    watch(_id, _after, next) {
      observer = next;
      return () => {};
    },
  };
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () =>
      root.render(
        createElement(RunHistoryView, {
          runId: detail.runId,
          source,
          workers: [],
        })
      )
    );
    await flushEffects();
    await check({
      container,
      retry: async () => {
        assert.ok(observer);
        await act(async () => observer!.page(page(events.slice(4, 5))));
      },
      finish: async () => {
        assert.ok(observer);
        await act(async () => observer!.page(page(events.slice(5))));
      },
    });
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
}

async function switchToSteps(container: HTMLElement) {
  const button = [...container.querySelectorAll<HTMLButtonElement>('.view-toggle button')].find(
    (candidate) => candidate.textContent === 'Steps'
  );
  assert.ok(button);
  await act(async () => button.click());
}

async function openReview(container: HTMLElement) {
  const button = container.querySelector<HTMLButtonElement>('button.story-step-head');
  assert.ok(button);
  await act(async () => button.click());
}

function assertFirstAttempt(container: HTMLElement) {
  const row = container.querySelector('.story-step.open');
  assert.ok(row, 'the grouped retry row is open');
  const detail = row.querySelector('.step-detail');
  assert.match(detail?.textContent ?? '', /First input/);
  assert.match(detail?.textContent ?? '', /First transcript/);
  assert.match(detail?.textContent ?? '', /First result/);
  assert.doesNotMatch(detail?.textContent ?? '', /Second (input|transcript|result)/);
  return row;
}

test('an earlier attempt selected in Graph opens its grouped Steps row and stays selected across views', async (t) => {
  await withRetriedSteps(t, async ({ container }) => {
    await openReview(container);
    const graph = container.querySelector<HTMLButtonElement>('.step-link');
    assert.ok(graph);
    await act(async () => graph.click());
    const attempts = container.querySelectorAll<HTMLButtonElement>('.execution-stop');
    assert.equal(attempts.length, 2);
    await act(async () => attempts[0].click());
    await switchToSteps(container);
    assertFirstAttempt(container);
    assert.match(container.querySelector('button.story-step-head')!.textContent!, /2 attempts/);
    const selector = container.querySelector<HTMLSelectElement>('select[aria-label="Attempt"]');
    assert.ok(selector);
    assert.equal(selector.value, 'attempt-17');
    await act(async () => {
      selector.value = 'attempt-91';
      selector.dispatchEvent(new window.Event('change', { bubbles: true }));
    });
    assert.match(container.querySelector('.step-detail')!.textContent!, /Second transcript/);
    assert.doesNotMatch(container.querySelector('.step-detail')!.textContent!, /First transcript/);
    await act(async () => container.querySelector<HTMLButtonElement>('.step-link')!.click());
    assert.match(
      container.querySelector('.execution-stop[aria-pressed="true"]')!.textContent!,
      /Attempt 2/
    );
  });
});

test('a live retry keeps the existing row open with its selected attempt and keeps following', async (t) => {
  await withRetriedSteps(
    t,
    async ({ container, retry, finish }) => {
      await openReview(container);
      const row = assertFirstAttempt(container);
      await retry();
      assert.equal(assertFirstAttempt(container), row, 'the same row survives the retry');
      assert.ok(row.classList.contains('state-running'));
      assert.match(row.textContent!, /2 attempts/);
      await finish();
      assert.equal(assertFirstAttempt(container), row, 'settlement preserves the selected attempt');
      assert.equal(
        container.querySelector('.history-live')?.getAttribute('title'),
        'Following latest events'
      );
      await act(async () => container.querySelector<HTMLButtonElement>('.step-link')!.click());
      assert.match(container.querySelector('.invocation-detail')!.textContent!, /First transcript/);
      await switchToSteps(container);
      assertFirstAttempt(container);
    },
    true
  );
});

test('expanded Steps results keep each verifier signal name beside its recorded value', async (t) => {
  await withRetriedSteps(t, async ({ container }) => {
    await openReview(container);
    const signals = [...container.querySelectorAll('.step-section')].find((section) =>
      section.querySelector('summary')?.textContent?.startsWith('Signals')
    );
    assert.ok(signals, 'the full signal record is available without leaving Steps');
    assert.deepEqual(
      [...signals.querySelectorAll('.step-row')].map((row) => [
        row.querySelector('dt')?.textContent,
        row.querySelector('dd')?.textContent,
      ]),
      [
        ['Code review', 'accepted'],
        ['Test review', 'rejected'],
      ]
    );
  });
});
