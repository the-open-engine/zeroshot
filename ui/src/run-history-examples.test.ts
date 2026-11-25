import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import type { Document } from './domain';
import { projectHistory, historyMoments, type HistoryEvent, type RunDetail } from './run-history';
import { compactWorkflow } from './workflow-display';
import { projectWorkflow } from './workflow-projection';
import { collapsedWorkflowGroups } from './workflow-observation';
import { appendHistory, createExampleRunHistory, readRunDetail } from './run-history-source';
import { projectStory } from './run-story';

const wire = readFileSync(new URL('../public/history-examples.json', import.meta.url), 'utf8');
const source = createExampleRunHistory(
  new URL('https://example.test/ui/'),
  async () => new Response(wire)
);
const examples: { detail: RunDetail; events: HistoryEvent[] }[] = [];
for (const row of (await source.list()).runs) {
  const detail = await source.detail(row.runId);
  let cursor = detail.history.initialCursor;
  let events: HistoryEvent[] = [];
  for (;;) {
    const page = await source.page(row.runId, cursor);
    events = appendHistory(events, page, cursor);
    if (page.complete) break;
    assert.notEqual(cursor, page.nextCursor);
    cursor = page.nextCursor;
  }
  examples.push({ detail, events });
}
test('eleven explicitly simulated histories retain renderable immutable definitions', () => {
  assert.equal(examples.length, 11);
  assert.equal(new Set(examples.map(({ detail }) => detail.runId)).size, 11);
  for (const { detail, events } of examples) {
    assert.equal(readRunDetail(detail, detail.runId), detail);
    assert.equal(detail.example, true);
    const document: Document = { name: detail.runId, graph: detail.graph, runtime: detail.runtime };
    for (const collapsed of [new Set<string>(), collapsedWorkflowGroups(document.graph.root)]) {
      const graph = compactWorkflow(projectWorkflow(document, collapsed));
      const ids = new Set(graph.nodes.map(({ id }) => id));
      assert.ok(ids.size > 0, detail.title);
      for (const edge of graph.edges) {
        assert.ok(ids.has(edge.source), edge.source);
        assert.ok(ids.has(edge.target), edge.target);
      }
    }
    for (const moment of historyMoments(events)) {
      const view = projectHistory(document, events, moment.index);
      for (const invocation of view.invocations) {
        assert.ok(invocation.start <= moment.index);
        assert.ok(invocation.logs.every((log) => log.index <= moment.index));
        if (invocation.end !== undefined) assert.ok(invocation.end <= moment.index);
      }
      if (moment.index < events.length - 1)
        assert.equal(view.terminal, undefined, `${detail.title} leaked its terminal`);
    }
    // Paused runs record no terminal; their detail says so with null.
    const final = projectHistory(document, events, events.length - 1);
    assert.deepEqual(final.terminal ?? null, detail.terminal ?? null);
    // A run that ended suspended runs nothing and queues nothing.
    if (detail.phase === 'suspended')
      for (const [name, node] of Object.entries(final.nodes))
        assert.ok(!['running', 'queued'].includes(node.state), `${detail.title}: ${name}`);
  }
});

test('Security patch approval shows both answers and handoffs in one step history and graph replay', async () => {
  const rows = (await source.list()).runs.filter((row) =>
    row.title.startsWith('Security patch approval')
  );
  assert.equal(rows.length, 1);
  const { detail, events } = examples.find((example) => example.detail.runId === rows[0].runId)!;
  const parts = detail.parts!;
  assert.equal(parts.length, 3);
  assert.equal(rows[0].currentPartId, parts[2]);
  assert.equal(rows[0].workspaceRecovery?.successorRunId, undefined);
  assert.equal((await source.detail(parts[1])).runId, detail.runId);
  const document = { name: detail.runId, graph: detail.graph, runtime: detail.runtime };
  const firstPause = events.findIndex((event) => event.event.kind === 'suspended');
  const paused = projectHistory(document, events, firstPause);
  assert.equal(paused.nodes.approval.state, 'suspended');
  assert.equal(paused.invocations.filter((work) => work.request).length, 1);
  const final = projectHistory(document, events, events.length - 1);
  assert.equal(final.terminal?.status, 'succeeded');
  assert.equal(final.nodes.deliver.state, 'succeeded');
  const answers = final.invocations.flatMap((work) => work.request?.resolution ?? []);
  assert.deepEqual(
    answers.map((answer) => answer.kind === 'answer' && answer.signals.verdict),
    ['rejected', 'accepted']
  );
  const story = projectStory(
    document,
    final.invocations,
    final.handoffs,
    final.clock.at,
    final.allControls
  );
  assert.deepEqual(
    final.handoffs.map((handoff) => handoff.from),
    parts.slice(0, 2)
  );
  assert.equal(story.filter((entry) => entry.kind === 'continued').length, 2);
});

test('the approval example can resume at every retained position inside each part', async () => {
  const { detail } = examples.find(({ detail }) => detail.title === 'Security patch approval')!;
  for (const part of detail.parts!) {
    const complete = await source.page(detail.runId, `${part}@v2:0`);
    const whole = appendHistory([], complete);
    const head = Number(complete.partHeadCursor!.slice(3));
    for (let seq = 0; seq <= head; seq++) {
      const cursor = `${part}@v2:${seq}`;
      const page = await source.page(detail.runId, cursor);
      const expected = whole.filter((event) => Number(event.partCursor!.slice(3)) > seq);
      assert.deepEqual(appendHistory([], page, cursor), expected);
      assert.equal(page.nextCursor, complete.nextCursor);
    }
    await assert.rejects(source.page(detail.runId, `${part}@v2:${head + 1}`));
  }
});

test('examples exercise maps, retries through a review loop, long output, and an authored failure', () => {
  const onboarding = examples.find(({ detail }) => detail.runId.includes('customer-onboarding'))!;
  assert.deepEqual((onboarding.detail.initialInput as any).products, []);
  const editorial = examples.find(({ detail }) => detail.runId.includes('editorial'))!;
  const starts = editorial.events.filter(({ event }) => event.kind === 'node_started');
  assert.ok(
    new Set(starts.map(({ event }) => JSON.stringify(event.occurrence?.mapIndices))).size >= 3
  );
  assert.ok(editorial.events.some(({ event }) => (event.line?.length ?? 0) > 12000));
  const repeated = starts.map(({ event }) => event.reference!.node);
  assert.ok(new Set(repeated).size < repeated.length);
  const failed = examples.find(({ detail }) => detail.runId.includes('flaky-test'))!;
  assert.equal(failed.detail.terminal?.status, 'failed');
  assert.ok(failed.events.some(({ event }) => event.completion?.outcome.code === 'timeout'));
});
