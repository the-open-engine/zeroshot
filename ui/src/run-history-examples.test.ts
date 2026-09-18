import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import type { Document } from './domain';
import { projectHistory, historyMoments, type HistoryEvent, type RunDetail } from './run-history';
import { compactWorkflow } from './workflow-display';
import { projectWorkflow } from './workflow-projection';
import { collapsedWorkflowGroups } from './workflow-observation';
import { appendHistory, readRunDetail } from './run-history-source';

const examples: { detail: RunDetail; events: HistoryEvent[] }[] = JSON.parse(
  readFileSync(new URL('../public/history-examples.json', import.meta.url), 'utf8')
);
test('ten explicitly simulated histories retain renderable immutable definitions', () => {
  assert.equal(examples.length, 10);
  assert.equal(new Set(examples.map(({ detail }) => detail.runId)).size, 10);
  for (const { detail, events } of examples) {
    assert.equal(readRunDetail(detail, detail.runId), detail);
    assert.equal(detail.example, true);
    assert.ok(detail.runId.startsWith('example-'));
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
    const merged = appendHistory([], {
      events,
      nextCursor: events.at(-1)!.cursor,
      headCursor: detail.history.cursor,
      complete: true,
    });
    assert.equal(merged.length, events.length);
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
    assert.deepEqual(projectHistory(document, events, events.length - 1).terminal, detail.terminal);
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
