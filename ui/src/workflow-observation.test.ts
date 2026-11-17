import test from 'node:test';
import assert from 'node:assert/strict';
import type { GraphNode } from './domain';
import {
  collapsedRunGroups,
  collapsedWorkflowGroups,
  currentRunWork,
  revealRunWork,
  revealWorkflowNode,
} from './workflow-observation';
import { projectWorkflow } from './workflow-projection';
import type { Document } from './domain';

const graph: GraphNode = {
  kind: 'seq',
  name: 'run',
  children: [
    { kind: 'step', name: 'prepare' },
    {
      kind: 'loop',
      name: 'review_rounds',
      maxIterations: 3,
      body: {
        kind: 'seq',
        name: 'review_round',
        children: [
          { kind: 'step', name: 'write' },
          {
            kind: 'par',
            name: 'reviews',
            join: { kind: 'all' },
            branches: [
              { kind: 'verifier', name: 'check_api' },
              { kind: 'verifier', name: 'check_tests' },
            ],
          },
        ],
      },
    },
    {
      kind: 'map',
      name: 'publish',
      body: { kind: 'step', name: 'release' },
    },
  ],
};

test('history flattens every sequence and initially folds semantic groups', () => {
  const collapsed = collapsedWorkflowGroups(graph);
  assert.deepEqual([...collapsed], ['review_rounds', 'reviews', 'publish']);
  const projection = projectWorkflow({ graph: { root: graph } } as Document, collapsed);
  assert.deepEqual(
    projection.nodes.filter((node) => node.role !== 'completion').map((node) => node.owner),
    ['prepare', 'review_rounds', 'publish']
  );
  assert.equal(projection.regions.length, 0);
  collapsed.delete('review_rounds');
  const expanded = projectWorkflow({ graph: { root: graph } } as Document, collapsed);
  assert.ok(expanded.nodes.some((node) => node.owner === 'write'));
  assert.ok(!expanded.nodes.some((node) => node.owner === 'review_round'));
});

test('a non-sequence root is a collapsed group, and leaves stay visible', () => {
  assert.deepEqual([...collapsedWorkflowGroups(graph.children[1])], ['review_rounds', 'reviews']);
  assert.deepEqual([...collapsedWorkflowGroups(graph.children[0])], []);
});

test('explicit node navigation reveals only ancestors and preserves other manual folds', () => {
  const collapsed = collapsedWorkflowGroups(graph);
  const revealed = revealWorkflowNode(collapsed, graph, 'check_tests');
  assert.deepEqual([...revealed], ['publish']);
  assert.deepEqual([...collapsed], ['review_rounds', 'reviews', 'publish']);
  assert.deepEqual([...revealWorkflowNode(collapsed, graph, 'reviews')], ['reviews', 'publish']);
  assert.deepEqual([...revealWorkflowNode(collapsed, graph, 'missing')], [...collapsed]);
});

test('a run opens with only the groups holding current or failed work expanded', () => {
  assert.deepEqual([...collapsedRunGroups(graph)], [...collapsedWorkflowGroups(graph)]);
  const failed = collapsedRunGroups(graph, { check_api: { state: 'failed' } });
  assert.equal(failed.has('review_rounds'), false);
  assert.equal(failed.has('reviews'), false);
  const writing = collapsedRunGroups(graph, {
    write: { state: 'running' },
    check_api: { state: 'succeeded' },
  });
  assert.equal(writing.has('review_rounds'), false);
  assert.equal(writing.has('reviews'), true);
});

test('work that becomes current later unfolds its groups without refolding others', () => {
  const folded = collapsedRunGroups(graph);
  assert.deepEqual(currentRunWork(graph), []);
  // Nothing current yet: the same set comes back, so the canvas does not lay out again.
  assert.equal(revealRunWork(folded, graph, currentRunWork(graph)), folded);
  const later = revealRunWork(
    folded,
    graph,
    currentRunWork(graph, { check_api: { state: 'failed' }, write: { state: 'succeeded' } })
  );
  assert.equal(later.has('reviews'), false);
  assert.equal(revealRunWork(later, graph, ['check_api']), later);
});
