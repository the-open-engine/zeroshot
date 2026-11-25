import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { act, createElement } from 'react';
import { ApiError, createApiClient } from './api';
import {
  appendHistory,
  createRunHistorySource,
  readRunDetail,
  type RunHistoryReader,
} from './run-history-source';
import { groupedControlPage } from './grouped-history';
import { projectHistory, type HistoryEvent, type HistoryPage, type RunDetail } from './run-history';
import { mergeRunPage, runMenuStatus, searchRunMenu } from './run-menu';
import { projectStory } from './run-story';
import { runDetailFixture } from './test-support';
import {
  clickButton,
  flushEffects,
  installBrowser,
  renderedText,
  showGraph,
  stubWorker,
  viewerModule,
} from './run-history-test-support';
import type { HistoryObserver } from './history-stream';

const root = '0199f33f-3b44-7d21-9000-000000000031';
const child = '0199f33f-3b44-7d21-9000-000000000032';
const page = (
  part: string,
  current: string,
  after: number,
  events: HistoryEvent['event'][],
  head: number
): HistoryPage => {
  const afterCursor = `v2:${after}`,
    next = `v2:${after + events.length}`;
  return {
    runId: root,
    partId: part,
    currentPartId: current,
    partAfterCursor: afterCursor,
    partNextCursor: next,
    partHeadCursor: part === current ? `v2:${head}` : next,
    nextCursor: part === current ? `${part}@${next}` : `${current}@v2:0`,
    headCursor: `${current}@v2:${head}`,
    complete: part === current && after + events.length === head,
    observation: { state: 'collecting' },
    events: events.map((event, index) => ({
      partId: part,
      cursor: `v2:${after + index + 1}`,
      runCursor: `${part}@v2:${after + index + 1}`,
      event,
    })),
  };
};
const continuation: HistoryEvent['event'] = {
  kind: 'vector',
  vector: {
    id: 'continue',
    kind: 'continuation',
    runId: child,
    resumedFrom: root,
    checkpointId: 'checkpoint-2',
  },
};
const suspended: HistoryEvent['event'] = {
  kind: 'suspended',
  suspension: { reason: 'command', node: 'work', createdAt: 1234 },
};
const terminal: HistoryEvent['event'] = {
  kind: 'terminal',
  result: { status: 'succeeded', output: null },
};
const ref = (part: string, node: string, id: string) => ({
  runId: part,
  node,
  execution: id,
  nodeInstance: id,
});
const doc = runDetailFixture(root);
doc.graph.root = {
  name: 'run',
  kind: 'seq',
  children: [
    { name: 'work', kind: 'step' },
    { name: 'later', kind: 'step' },
  ],
};
const definition = (head: string, phase: string): RunDetail => ({
  ...doc,
  partId: head,
  currentPartId: head,
  parts: head === root ? [root] : [root, child],
  phase,
  cursor: `${head}@v2:2`,
  history: { ...doc.history, initialCursor: `${root}@v2:0`, cursor: `${head}@v2:2` },
});

function importPriorWork(page: HistoryPage) {
  const [seed] = page.events.splice(1, 1);
  Object.assign(seed.event, {
    execution: {
      execution: '1',
      node_instance: '1',
      occurrence: { node: 'work', mapIndices: [] },
      state: { Settled: { outcome: { status: 'succeeded' } } },
    },
  });
  page.imports = [seed];
}

test('rewind seeds rebuild only the selected graph while Steps preserves original attempts and transcripts', () => {
  const first = page(
    root,
    child,
    0,
    [
      { kind: 'node_started', reference: ref(root, 'work', '1') },
      { kind: 'safe_log', execution: '1', line: 'before' },
      {
        kind: 'node_completed',
        completion: { reference: ref(root, 'work', '1'), outcome: { status: 'succeeded' } },
      },
      { kind: 'node_started', reference: ref(root, 'later', '2') },
      {
        kind: 'node_completed',
        completion: { reference: ref(root, 'later', '2'), outcome: { status: 'error' } },
      },
      { kind: 'terminal', result: { status: 'failed', reason: 'unhandled' } },
    ],
    5
  );
  const second = page(
    child,
    child,
    0,
    [
      continuation,
      { kind: 'prior_execution' },
      { kind: 'node_started', reference: ref(child, 'later', '2') },
      { kind: 'safe_log', execution: '2', line: 'retried' },
      terminal,
    ],
    5
  );
  importPriorWork(second);
  second.events[0].resumeReason = 'checkpoint';
  const prior = appendHistory([], first, `${root}@v2:0`);
  const history = appendHistory(prior, second, first.nextCursor);
  const document = { name: root, graph: doc.graph, runtime: doc.runtime };
  const before = projectHistory(document, history, prior.length - 1);
  assert.equal(before.nodes.later.state, 'failed');
  const rewind = projectHistory(document, history, prior.length + 1);
  assert.equal(rewind.nodes.work.state, 'succeeded');
  assert.ok(!rewind.nodes.later || rewind.nodes.later.state === 'idle');
  assert.equal(rewind.terminal, undefined);
  const done = projectHistory(document, history, history.length - 1);
  assert.equal(done.invocations.length, 3);
  assert.equal(new Set(done.invocations.map((work) => work.id)).size, 3);
  assert.equal(done.invocations[0].logs[0].text, 'before');
  assert.equal(done.invocations[2].logs[0].text, 'retried');
  assert.deepEqual(
    projectStory(document, done.invocations, done.handoffs)
      .filter((entry) => entry.kind === 'continued')
      .map((entry) => entry.label),
    ['Restored checkpoint checkpoint-2']
  );
  assert.equal(readRunDetail(definition(child, 'running'), child).runId, root);
  assert.throws(() => appendHistory(prior, second, `${root}@v2:0`), /gap/);
});

test('a copied vector with a reducer transition retains its native replay anchor', () => {
  const part = page(child, child, 0, [continuation], 1);
  part.events = [];
  part.omittedCursors = ['v2:1'];
  part.importedControl = [
    {
      partId: child,
      cursor: 'v2:1',
      runCursor: `${child}@v2:1`,
      visitId: 'visit',
      node: 'run',
      mapIndices: [],
      state: 'entered',
    },
  ];
  const normalized = groupedControlPage(part);
  assert.equal(normalized.events[0].event.kind, 'control_position');
  assert.equal(normalized.control?.[0].cursor, normalized.events[0].cursor);
});

async function renderViewer(
  t: TestContext,
  source: RunHistoryReader,
  runId = root,
  embedded = false
) {
  stubWorker(t);
  installBrowser(t);
  const { RunHistoryView } = await viewerModule(t);
  const { createRoot } = await import('react-dom/client');
  const container = document.createElement('div');
  document.body.append(container);
  const mount = createRoot(container);
  await act(async () =>
    mount.render(createElement(RunHistoryView, { runId, source, workers: [], embedded }))
  );
  await flushEffects();
  return {
    container,
    async close() {
      await act(async () => mount.unmount());
      container.remove();
    },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test('cold open replays retained steps while the head is queued and preserves a pinned graph on arrival', async (t) => {
  const pending = () =>
    Response.json(
      { code: 'history_pending', message: 'Preparing the current part' },
      { status: 503 }
    );
  const metadataRequested = deferred<void>();
  const headPage = deferred<HistoryPage>();
  const headDetail = deferred<RunDetail>();
  const first = page(
    root,
    child,
    0,
    [
      { kind: 'node_started', reference: ref(root, 'work', '1') },
      { kind: 'safe_log', execution: '1', line: 'retained compiler output' },
      suspended,
    ],
    0
  );
  const exact = {
    ...definition(child, 'suspended'),
    partId: root,
    partView: true,
    runtime: { ...doc.runtime, harness: 'pi', provider: 'anthropic' },
  };
  let opens = 0;
  const reads: (string | undefined)[] = [];
  const base = new URL('http://localhost/ui/api/');
  const fetcher: typeof fetch = async (url) => {
    const { pathname, searchParams } = new URL(String(url));
    if (pathname.endsWith('/history')) {
      assert.equal(pathname, base.pathname + 'runs/' + child + '/history');
      const after = searchParams.get('after') ?? undefined;
      reads.push(after);
      if (after === undefined) return reads.length === 1 ? pending() : Response.json(first);
      assert.equal(after, `${child}@v2:0`);
      return Response.json(await headPage.promise);
    }
    if (searchParams.has('partId')) {
      assert.equal(searchParams.get('partId'), root);
      assert.equal(pathname, base.pathname + 'runs/' + child);
      return Response.json(exact);
    }
    if (pathname === base.pathname + 'runs/' + child) {
      return pending();
    }
    assert.equal(pathname, base.pathname + 'runs/' + root);
    if (++opens === 1) return pending();
    metadataRequested.resolve();
    return Response.json(await headDetail.promise);
  };
  const source = createRunHistorySource(createApiClient(base, fetcher), base, fetcher);
  source.retryDelay = (cause) =>
    cause instanceof ApiError && cause.code === 'history_pending' ? 1 : undefined;
  const { container, close } = await renderViewer(t, source, child);
  try {
    assert.match(renderedText(container), /retained compiler output/);
    assert.deepEqual(reads, [undefined, undefined, `${child}@v2:0`]);
    await showGraph(container);
    await clickButton(container, 'Previous event');
    await clickButton(container, 'Show details');
    const position = () =>
      container.querySelector<HTMLInputElement>('[aria-label="History position"]')!.value;
    const pinned = position();
    assert.match(renderedText(container), /pi \/ anthropic/);
    await act(async () =>
      headPage.resolve({
        ...page(child, child, 0, [continuation, terminal], 2),
        finished: true,
        observation: { state: 'complete' },
      })
    );
    await metadataRequested.promise;
    await act(async () => headDetail.resolve(definition(child, 'finished')));
    await flushEffects();
    assert.equal(position(), pinned);
    assert.match(renderedText(container), /pi \/ anthropic/);
    await clickButton(container, 'Go to latest');
    assert.match(renderedText(container), /codex \/ openai/);
  } finally {
    await close();
  }
});

test('embedded Steps follows a committed handoff without a menu and preserves the original request in replay', async (t) => {
  const request = {
    id: 'ask',
    text: 'Ship it?',
    signals: { choice: ['yes'] },
    output: null,
    expiresAt: 65_000,
  };
  const node = 'approval';
  const reference = ref(root, node, '1');
  const suspension: RunDetail['suspension'] = {
    reason: 'request',
    node,
    createdAt: 5_000,
    request,
  };
  const prefix = page(
    root,
    child,
    0,
    [
      { kind: 'node_started', reference },
      { kind: 'request_deferred', reference },
      {
        kind: 'vector',
        vector: { kind: 'request', ...request, node, createdAt: suspension.createdAt },
      },
      { kind: 'suspended', suspension },
    ],
    0
  );
  const exact = {
    ...definition(child, 'suspended'),
    partView: true,
    partId: root,
    graph: {
      ...doc.graph,
      root: { kind: 'verifier' as const, name: node, worker: 'builtin.request.ask@1' },
    },
  };
  const pending = new Error('Successor ledger is pending');
  const source: RunHistoryReader = {
    detail: async () => {
      throw pending;
    },
    retryDelay: (error) => (error === pending ? 1 : undefined),
    forPart: () => ({ ...source, detail: async () => exact }),
    page: async (_, after) => (after ? deferred<HistoryPage>().promise : prefix),
    watch: () => () => {},
  };
  const { container, close } = await renderViewer(t, source, root, true);
  try {
    const summary = () => container.querySelector('.run-outcome-text')?.textContent;
    const question = container.querySelector('.story-step-head');
    assert.equal(summary(), 'Continuing');
    assert.ok(question instanceof HTMLElement);
    await act(async () => question.click());
    assert.match(renderedText(container), /Handed off/);
    assert.doesNotMatch(renderedText(container), /Waiting for an answer|Answer with/);
    await clickButton(container, 'Previous event');
    assert.match(summary()!, /Waiting for a person/);
    assert.match(renderedText(container), /Waiting for an answer/);
    assert.doesNotMatch(renderedText(container), /Handed off/);
    await clickButton(container, 'Go to latest');
    assert.equal(summary(), 'Continuing');
  } finally {
    await close();
  }
});

for (const failureAt of ['definition', 'retained page'] as const)
  test(`a refused ${failureAt} is surfaced without another archive lookup`, async (t) => {
    const reads: string[] = [];
    const base = new URL('http://localhost/ui/api/');
    const fetcher: typeof fetch = async (url) => {
      const path = new URL(String(url)).pathname;
      reads.push(path);
      if (failureAt === 'retained page' && reads.length === 1)
        return Response.json({ code: 'history_pending' }, { status: 503 });
      return Response.json(
        { code: 'forbidden', message: 'History access was refused.' },
        { status: 403 }
      );
    };
    const source = createRunHistorySource(createApiClient(base, fetcher), base, fetcher);
    const { container, close } = await renderViewer(t, source, child);
    try {
      assert.match(renderedText(container), /History access was refused/);
      const address = base.pathname + 'runs/' + child;
      assert.deepEqual(
        reads,
        failureAt === 'definition' ? [address] : [address, address + '/history']
      );
    } finally {
      await close();
    }
  });

for (const pinned of [false, true])
  test(`a ${pinned ? 'pinned replay' : 'live viewer'} receives a successor on the existing subscription`, async (t) => {
    let current = root;
    let observer: HistoryObserver | undefined;
    const subscriptions: string[] = [];
    const exactReads: string[] = [];
    const source: RunHistoryReader = {
      forPart(part) {
        return {
          ...source,
          async detail(id) {
            exactReads.push(part);
            assert.equal(id, root);
            return {
              ...definition(child, 'suspended'),
              partView: true,
              partId: part,
              runtime: { ...doc.runtime, harness: 'pi', provider: 'anthropic' },
            };
          },
        };
      },
      async detail(id) {
        assert.equal(id, root);
        return definition(current, current === root ? 'suspended' : 'finished');
      },
      async page(id, after) {
        assert.equal(id, root);
        assert.equal(after, `${root}@v2:0`);
        return page(root, root, 0, [{ kind: 'run_started' }, suspended], 2);
      },
      watch(id, after, receiver) {
        subscriptions.push(id + ' ' + after);
        observer = receiver;
        return () => {};
      },
    };
    const { container, close } = await renderViewer(t, source);
    try {
      await showGraph(container);
      assert.deepEqual(subscriptions, [root + ' ' + root + '@v2:2']);
      if (pinned) await clickButton(container, 'Previous event');
      const position = () =>
        container.querySelector<HTMLInputElement>('[aria-label="History position"]')!.value;
      const original = position();
      current = child;
      await act(async () => {
        observer!.page(page(root, child, 2, [], 2));
        const final = page(child, child, 0, [continuation, terminal], 2);
        final.finished = true;
        final.observation = { state: 'complete' };
        observer!.page(final);
      });
      await flushEffects();
      assert.equal(subscriptions.length, 1);
      assert.equal(container.querySelector('[aria-label="Run parts"]'), null);
      assert.doesNotMatch(renderedText(container), /gap or inconsistent|stopped advancing/);
      if (pinned) {
        assert.equal(position(), original);
        assert.deepEqual(exactReads, [root]);
        await clickButton(container, 'Show details');
        assert.match(renderedText(container), /pi \/ anthropic/);
      } else {
        assert.ok(Number(position()) > Number(original));
        assert.match(renderedText(container), /Succeeded/i);
      }
    } finally {
      await close();
    }
  });

test('menu refresh removes provisional rows across multiple confirmed handoffs but retains older pages', () => {
  const middle = '0199f33f-3b44-7d21-9000-000000000033';
  const other = '0199f33f-3b44-7d21-9000-000000000030';
  const summary = (id: string) => ({ ...definition(id, 'finished'), runId: id, terminal: null });
  const final = { ...summary(root), currentPartId: middle, phase: 'running' };
  const merged = mergeRunPage([summary(root), summary(child), summary(middle), summary(other)], {
    runs: [final],
    nextCursor: root,
  });
  assert.deepEqual(
    merged.map((run) => [run.runId, run.currentPartId]),
    [
      [root, middle],
      [other, other],
    ]
  );
});

test('run search finds the current part and offers exact lookup for a prior or unloaded ID', () => {
  const run = { ...definition(root, 'finished'), currentPartId: child, title: 'Release review' };
  const older = '0199f33f-3b44-7d21-9000-000000000033';
  for (const query of [root, child, 'RELEASE']) {
    assert.deepEqual(searchRunMenu([run], query), { matches: [run], lookupId: undefined });
  }
  assert.deepEqual(searchRunMenu([run], ' ' + older.toUpperCase() + ' '), {
    matches: [],
    lookupId: older,
  });
  assert.deepEqual(searchRunMenu([], older.slice(0, 8)), { matches: [], lookupId: undefined });
});

test('a failed successor’s verifier link opens its own log when an earlier part reused the cursor and execution', async (t) => {
  const terminal = {
    status: 'failed' as const,
    reason: 'unhandled',
    context: {
      verifiers: [
        {
          message: 'Verifier found a problem',
          partId: child,
          cursor: 'v2:2',
          runCursor: `${child}@v2:2`,
        },
      ],
    },
  };
  const first = page(
    root,
    child,
    0,
    [
      { kind: 'node_started', reference: ref(root, 'work', '1') },
      { kind: 'safe_log', execution: '1', line: 'first part transcript' },
      suspended,
    ],
    3
  );
  const last = page(
    child,
    child,
    0,
    [
      { kind: 'node_started', reference: ref(child, 'later', '1') },
      { kind: 'safe_log', execution: '1', line: 'child verifier transcript' },
      { kind: 'terminal', result: terminal },
    ],
    3
  );
  last.finished = true;
  last.observation = { state: 'complete' };
  const source: RunHistoryReader = {
    async detail() {
      return { ...definition(child, 'finished'), terminal, cursor: last.headCursor };
    },
    async page(_, after) {
      return after === first.nextCursor ? last : first;
    },
  };
  const { container, close } = await renderViewer(t, source);
  try {
    const button = Array.from(container.querySelectorAll('button')).find((button) =>
      button.textContent?.includes('View log · v2:2')
    );
    assert.ok(button);
    await act(async () => button.click());
    const open = container.querySelector('.story-step.open');
    assert.ok(open);
    assert.match(open.textContent!, /Later/);
    assert.match(open.textContent!, /child verifier transcript/);
    assert.doesNotMatch(open.textContent!, /first part transcript/);
  } finally {
    await close();
  }
});

test('Steps and Graph navigate between original work and a later checkpoint prerequisite', async (t) => {
  const first = page(
    root,
    child,
    0,
    [
      { kind: 'node_started', reference: ref(root, 'work', '1') },
      {
        kind: 'node_completed',
        completion: { reference: ref(root, 'work', '1'), outcome: { status: 'succeeded' } },
      },
      suspended,
    ],
    3
  );
  const last = page(child, child, 0, [continuation, { kind: 'prior_execution' }, terminal], 3);
  importPriorWork(last);
  last.finished = true;
  last.observation = { state: 'complete' };
  const source: RunHistoryReader = {
    async detail() {
      return { ...definition(child, 'finished'), terminal: terminal.result };
    },
    async page(_, after) {
      return after === first.nextCursor ? last : first;
    },
  };
  const { container, close } = await renderViewer(t, source);
  try {
    const work = () =>
      Array.from(container.querySelectorAll<HTMLButtonElement>('.story-step-head')).find((node) =>
        node.textContent?.includes('Work')
      )!;
    await act(async () => work().click());
    const link = Array.from(container.querySelectorAll('button')).find(
      (node) => node.textContent === 'Show in graph'
    )!;
    await act(async () => link.click());
    await flushEffects();
    assert.match(container.querySelector('[aria-label="Run inspector"]')!.textContent!, /Work/);
    const fraction = () =>
      container.querySelector<HTMLInputElement>('[aria-label="History position"]')!;
    assert.ok(Number(fraction().value) < Number(fraction().max));
    await clickButton(container, 'Go to latest');
    const steps = Array.from(container.querySelectorAll('button')).find(
      (node) => node.textContent === 'Steps'
    )!;
    await act(async () => steps.click());
    await flushEffects();
    assert.equal(work().getAttribute('aria-expanded'), 'true');
  } finally {
    await close();
  }
});

test('the run menu reports a queued successor before native history is available', () => {
  const queued = {
    ...runDetailFixture(),
    currentPartId: 'queued-part',
    phase: 'queued',
    historyAvailable: false,
    cursor: null,
  };
  assert.equal(runMenuStatus(queued), 'Queued');
  assert.equal(runMenuStatus({ ...queued, phase: 'unavailable' }), 'History unavailable');
});
