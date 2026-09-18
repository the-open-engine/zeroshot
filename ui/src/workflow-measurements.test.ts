import test from 'node:test';
import assert from 'node:assert/strict';
import { adoptUserNodes, nodeHasDimensions } from '@xyflow/system';
import type { Node, NodeChange } from '@xyflow/react';
import { compactWorkflow } from './workflow-display';
import { projectWorkflow } from './workflow-projection';
import type { Document } from './domain';
import { updateWorkflowMeasurements, withWorkflowDimensions } from './workflow-measurements';

const verifier = (name: string) => ({ kind: 'verifier', name });
const error = (name: string) => ({
  kind: 'in',
  value: { source: 'error', name, field: null },
  labels: ['crash'],
});

test('guard edits retain visible node sizes and handle bounds across controlled React Flow adoption', () => {
  // Structural shape of the exported flaky-test investigation: map, guarded loop,
  // and a later report-error outcome. Profile prompts/runtime are irrelevant to layout.
  const document = {
    name: 'measurement-regression',
    graph: {
      root: {
        kind: 'seq',
        name: 'run',
        children: [
          {
            kind: 'map',
            name: 'suites',
            maxItems: 10,
            body: verifier('investigate'),
            over: { source: 'state', path: ['suites'] },
          },
          {
            kind: 'choice',
            name: 'investigation_outcome',
            branches: [{ when: error('investigate'), node: { kind: 'fail', name: 'failed' } }],
            otherwise: {
              kind: 'seq',
              name: 'review_report',
              children: [
                {
                  kind: 'loop',
                  name: 'report_rounds',
                  maxIterations: 3,
                  until: error('review'),
                  body: {
                    kind: 'seq',
                    name: 'round',
                    children: [verifier('compose'), verifier('review')],
                  },
                },
                {
                  kind: 'choice',
                  name: 'report_outcome',
                  branches: [
                    { when: error('compose'), node: { kind: 'fail', name: 'report_failed' } },
                  ],
                },
              ],
            },
          },
        ],
      },
    },
  } as unknown as Document;
  function nodes(doc: Document, measurements: any): Node[] {
    const projection = compactWorkflow(projectWorkflow(doc));
    return [...projection.nodes, ...projection.regions].map((item, index) =>
      withWorkflowDimensions(
        {
          id: item.id,
          data: { document: doc },
          position: { x: index * 210, y: 0 },
        },
        { width: 190, height: 104 },
        measurements
      )
    );
  }
  const lookup = new Map(),
    parents = new Map();
  const initial = nodes(document, {});
  adoptUserNodes(initial, lookup, parents);
  assert.ok(
    [...lookup.values()].every(nodeHasDimensions),
    'known sizes render before ResizeObserver'
  );
  const changes: NodeChange[] = initial.map((node) => ({
    type: 'dimensions',
    id: node.id,
    dimensions: { width: 190, height: 104 },
  }));
  const measurements = updateWorkflowMeasurements({}, changes);
  for (const node of lookup.values()) {
    node.measured = measurements[node.id];
    node.internals.handleBounds = { source: [], target: [] };
  }
  const edited = structuredClone(document);
  edited.graph.root.children[1].otherwise.children[1].branches[0].when = {
    kind: 'any',
    guards: [error('compose'), error('review')],
  };
  const updated = nodes(edited, measurements);
  assert.deepEqual(
    updated.map((node) => node.id),
    initial.map((node) => node.id)
  );
  adoptUserNodes(updated, lookup, parents);
  assert.ok(
    [...lookup.values()].every(nodeHasDimensions),
    'guard-only rerenders cannot hide nodes'
  );
  assert.ok(
    [...lookup.values()].every((node) => node.internals.handleBounds),
    'edges retain measured handles'
  );
});

test('measurement changes are immutable, idempotent and do not react to position or invalid size updates', () => {
  const initial = { activity: { width: 190, height: 104 } };
  const unchanged: NodeChange[] = [
    { type: 'dimensions', id: 'activity', dimensions: { width: 190, height: 104 } },
    { type: 'dimensions', id: 'invalid', dimensions: { width: NaN, height: 0 } },
    { type: 'position', id: 'activity', position: { x: 10, y: 10 } },
  ];
  assert.equal(updateWorkflowMeasurements(initial, unchanged), initial);
  const next = updateWorkflowMeasurements(initial, [
    { type: 'dimensions', id: 'activity', dimensions: { width: 200, height: 110 } },
    { type: 'dimensions', id: 'activity', dimensions: { width: 190, height: 104 } },
    { type: 'dimensions', id: 'region', dimensions: { width: 650, height: 400 } },
  ]);
  assert.deepEqual(next, { ...initial, region: { width: 650, height: 400 } });
  assert.deepEqual(initial, { activity: { width: 190, height: 104 } });
});
