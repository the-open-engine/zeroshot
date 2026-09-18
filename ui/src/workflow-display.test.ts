import test from 'node:test';
import assert from 'node:assert/strict';
import type { Document, GraphNode } from './domain';
import { projectWorkflow, type WorkflowEdge, type WorkflowProjection } from './workflow-projection';
import { compactWorkflow, shortWorkflowLabel } from './workflow-display';

const agent = (name: string): GraphNode => ({ kind: 'step', name });
const done = (name: string): GraphNode => ({ kind: 'succeed', name });
const loop = (name: string, body: GraphNode): GraphNode => ({
  kind: 'loop',
  name,
  body,
  maxIterations: 10,
});
const seq = (...children: GraphNode[]): GraphNode => ({ kind: 'seq', name: 'root', children });
const doc = (root: GraphNode): Document => ({
  name: 'test',
  graph: { profile: 'test', initialInput: { kind: 'null' }, root, policy: {} },
  runtime: { harness: '', provider: '', size: 'small', nodes: {} },
});
const edge = (label?: string, extra: Partial<WorkflowEdge> = {}): WorkflowEdge => ({
  id: 'workflow:edge:test',
  source: 'source',
  target: 'target',
  label,
  ...extra,
});
const errors = 'timeout | crash | malformed | refusal';

test('compact captions recognize only complete common guards and retain priority', () => {
  assert.equal(shortWorkflowLabel(edge(`1. Worker error is ${errors}`)), '1. Worker has an error');
  assert.equal(
    shortWorkflowLabel(edge(`1. (Acceptance error is ${errors}) OR (Code error is ${errors})`)),
    '1. Any error'
  );
  assert.equal(
    shortWorkflowLabel(
      edge('2. (Acceptance · verdict is accepted) AND (Code · verdict is accepted)')
    ),
    '2. All accepted'
  );
  assert.equal(
    shortWorkflowLabel(edge('OTHERWISE · no earlier match', { otherwise: true })),
    'Otherwise'
  );
  assert.equal(shortWorkflowLabel(edge(undefined)), undefined);
  for (const label of [
    '1. Worker error is crash',
    '1. Worker error is timeout | crash | malformed | crash',
    `1. (Acceptance error is ${errors}) AND (Code error is ${errors})`,
    `1. (Acceptance error is ${errors}) OR (Code error is timeout)`,
    '2. (Acceptance · verdict is accepted) OR (Code · verdict is accepted)',
    '2. (Acceptance · verdict is accepted) AND (Code · verdict is rejected)',
    '2. (Acceptance · verdict is accepted) AND (Code · status is accepted)',
    `1. (Acceptance error is ${errors}) OR (Code error is timeout | crash…)`,
    'NOT ((Acceptance · verdict is accepted) AND (Code · verdict is accepted))',
  ])
    assert.equal(shortWorkflowLabel(edge(label)), label);
});

test('loop entry removal preserves incoming choice metadata and complete labels', () => {
  const choice: GraphNode = {
    kind: 'choice',
    name: 'decision',
    branches: [
      {
        when: {
          kind: 'in',
          value: { name: 'worker', source: 'error', field: null },
          labels: ['timeout', 'crash', 'malformed', 'refusal'],
        },
        node: loop('retry', agent('review')),
      },
    ],
    otherwise: done('finish'),
  };
  const original = projectWorkflow(doc(seq(agent('worker'), choice)));
  const before = structuredClone(original);
  const start = original.nodes.find((node) => node.role === 'loop-start')!;
  const branch = original.edges.find((entry) => entry.target === start.id)!;
  const compact = compactWorkflow(original);
  assert.equal(
    compact.nodes.some((node) => node.role === 'loop-start'),
    false
  );
  assert.equal(
    compact.nodes.some((node) => node.role === 'loop-end'),
    true
  );
  const redirected = compact.edges.find((entry) => entry.id === branch.id)!;
  assert.equal(redirected.target, compact.nodes.find((node) => node.owner === 'review')!.id);
  assert.equal(redirected.owner, 'decision');
  assert.equal(redirected.branchIndex, 0);
  assert.equal(redirected.label, branch.label);
  assert.equal(redirected.fullLabel, branch.label);
  assert.equal(redirected.shortLabel, '1. Worker has an error');
  assert.ok(compact.regions.every((region) => !region.nodeIds.includes(start.id)));
  assert.deepEqual(original, before);
});

test('nested loop entries redirect transitively without adding terminal continuation', () => {
  const original = projectWorkflow(
    doc(seq(agent('worker'), loop('outer', loop('inner', done('finish')))))
  );
  const compact = compactWorkflow(original);
  assert.equal(
    compact.nodes.some((node) => node.role === 'loop-start' || node.role === 'loop-end'),
    false
  );
  const worker = compact.nodes.find((node) => node.owner === 'worker')!;
  const finish = compact.nodes.find((node) => node.owner === 'finish')!;
  assert.ok(
    compact.edges.some((entry) => entry.source === worker.id && entry.target === finish.id)
  );
  assert.equal(
    compact.edges.some((entry) => entry.source === finish.id),
    false
  );
  assert.ok(compact.regions.every((region) => region.nodeIds.includes(finish.id)));
});

test('otherwise and repeat edges keep their authored meaning when targeting a loop entry', () => {
  const decision: GraphNode = {
    kind: 'choice',
    name: 'pick',
    branches: [{ when: {}, node: done('done') }],
    otherwise: loop('retry', agent('work')),
  };
  const original = projectWorkflow(doc(decision));
  const compact = compactWorkflow(original);
  const fallback = original.edges.find((entry) => entry.otherwise)!;
  assert.equal(compact.edges.find((entry) => entry.id === fallback.id)?.otherwise, true);
  assert.equal(compact.edges.find((entry) => entry.id === fallback.id)?.label, fallback.label);
  const repeat = original.edges.find((entry) => entry.repeat)!;
  assert.deepEqual(
    compact.edges.find((entry) => entry.id === repeat.id),
    { ...repeat, fullLabel: repeat.label, shortLabel: repeat.label }
  );
});

test('map starts and empty/overflow bypasses survive unchanged, including nested loops', () => {
  const map: GraphNode = {
    kind: 'map',
    name: 'each',
    maxItems: 5,
    over: { source: 'state', path: ['tasks'] },
    body: loop('per_item', agent('work')),
  };
  const original = projectWorkflow(doc(map));
  const compact = compactWorkflow(original);
  const start = original.nodes.find((node) => node.role === 'map-start')!;
  assert.ok(compact.nodes.some((node) => node.id === start.id));
  const bypass = original.edges.find((entry) => entry.source === start.id && entry.secondary)!;
  const after = compact.edges.find((entry) => entry.id === bypass.id)!;
  assert.equal(after.target, bypass.target);
  assert.equal(after.label, bypass.label);
  assert.equal(after.shortLabel, bypass.label);
  assert.equal(after.secondary, true);
  const itemEntry = compact.edges.find((entry) => entry.source === start.id && !entry.secondary)!;
  assert.equal(itemEntry.target, compact.nodes.find((node) => node.owner === 'work')!.id);
  assert.equal(itemEntry.label, 'Each item');
});

test('unexpected entry structures and cycles retain their markers instead of losing semantics', () => {
  const original = projectWorkflow(doc(loop('retry', agent('work'))));
  const start = original.nodes.find((node) => node.role === 'loop-start')!;
  const noRegion = compactWorkflow({ ...original, regions: [] });
  assert.ok(noRegion.nodes.some((node) => node.id === start.id));
  const condition = structuredClone(original);
  condition.edges[0].label = 'An authored condition';
  assert.ok(compactWorkflow(condition).nodes.some((node) => node.id === start.id));
  const cycle: WorkflowProjection = structuredClone(original);
  cycle.edges.find((entry) => entry.source === start.id)!.target = start.id;
  assert.ok(compactWorkflow(cycle).nodes.some((node) => node.id === start.id));
});

test('compaction is repeatable and does not change any surviving original edge label', () => {
  const original = projectWorkflow(
    doc(seq(agent('worker'), loop('repeat', agent('review')), done('done')))
  );
  const compact = compactWorkflow(original);
  assert.deepEqual(compactWorkflow(compact), compact);
  for (const after of compact.edges)
    assert.equal(after.label, original.edges.find((entry) => entry.id === after.id)!.label);
  const ids = new Set(compact.nodes.map((node) => node.id));
  assert.ok(compact.edges.every((entry) => ids.has(entry.source) && ids.has(entry.target)));
  assert.ok(compact.regions.every((region) => region.nodeIds.every((id) => ids.has(id))));
});
