import test from 'node:test';
import assert from 'node:assert/strict';
import type { GraphNode } from './domain';
import { collapsedWorkflowGroups, revealWorkflowNode } from './workflow-observation';
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
