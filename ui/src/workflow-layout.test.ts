import test from 'node:test';
import assert from 'node:assert/strict';
import ELK from 'elkjs/lib/elk.bundled.js';
import type { Document, GraphNode } from './domain';
import { compactWorkflow } from './workflow-display';
import { projectWorkflow, type WorkflowProjection } from './workflow-projection';
import {
  buildWorkflowLayout,
  readWorkflowLayout,
  type WorkflowDimensions,
  type WorkflowLayoutResult,
  type WorkflowRectangle,
} from './workflow-layout-model';

const agent = (name: string): GraphNode => ({ kind: 'step', name });
const done = (name: string): GraphNode => ({ kind: 'succeed', name });
const fail = (name: string): GraphNode => ({ kind: 'fail', name, reason: 'layout_failure' });
const seq = (name: string, children: GraphNode[]): GraphNode => ({ kind: 'seq', name, children });
const loop = (name: string, body: GraphNode): GraphNode => ({
  kind: 'loop',
  name,
  body,
  maxIterations: 10,
});
const error = (name: string) => ({
  kind: 'in',
  value: { name, source: 'error', field: null },
  labels: ['timeout', 'crash', 'malformed', 'refusal'],
});
const accepted = (name: string) => ({
  kind: 'in',
  value: { name, source: 'signal', field: 'verdict' },
  labels: ['accepted'],
});
function doc(root: GraphNode): Document {
  return {
    name: 'layout',
    graph: {
      profile: 'openengine.graph.full/v1',
      root,
      initialInput: { kind: 'null' },
      policy: {},
    },
    runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
  };
}
function dimensions(projection: WorkflowProjection): WorkflowDimensions {
  return Object.fromEntries(
    projection.nodes.map((node) => {
      const size = ['fork', 'join', 'loop-start', 'loop-end', 'map-end'].includes(node.role)
        ? { width: 28, height: 28 }
        : node.role === 'completion'
          ? { width: 132, height: 40 }
          : node.role === 'activity'
            ? { width: 190, height: 104 }
            : node.role === 'decision'
              ? { width: 164, height: 72 }
              : { width: 190, height: 104 };
      return [node.id, size];
    })
  );
}
async function arrange(root: GraphNode) {
  const projection = compactWorkflow(projectWorkflow(doc(root)));
  const sizes = dimensions(projection);
  const before = structuredClone({ projection, sizes });
  const elk = new ELK();
  // The bundled Node adapter runs in process and has no worker to terminate.
  const raw = await elk.layout(buildWorkflowLayout(projection, sizes));
  const layout = readWorkflowLayout(raw, projection);
  assert.deepEqual({ projection, sizes }, before, 'layout leaves projection and dimensions intact');
  assert.equal(Object.keys(layout.positions).length, projection.nodes.length);
  assert.equal(Object.keys(layout.regions).length, projection.regions.length);
  for (const position of Object.values(layout.positions)) {
    assert.ok(Number.isFinite(position.x) && Number.isFinite(position.y));
  }
  for (const region of Object.values(layout.regions)) {
    assert.ok(Number.isFinite(region.x) && Number.isFinite(region.y));
    assert.ok(region.width > 0 && region.height > 0);
  }
  return { projection, sizes, layout };
}
function regionOf(projection: WorkflowProjection, layout: WorkflowLayoutResult, owner: string) {
  const region = projection.regions.find((entry) => entry.owner === owner);
  assert.ok(region, `${owner} region is projected`);
  return layout.regions[region.id];
}
function nodeOf(
  projection: WorkflowProjection,
  sizes: WorkflowDimensions,
  layout: WorkflowLayoutResult,
  owner: string
): WorkflowRectangle {
  const node = projection.nodes.find(
    (entry) => entry.owner === owner && ['activity', 'completion'].includes(entry.role)
  );
  assert.ok(node, `${owner} activity or endpoint is projected`);
  return { ...layout.positions[node.id], ...sizes[node.id] };
}
function contains(outer: WorkflowRectangle, inner: WorkflowRectangle) {
  return (
    inner.x >= outer.x &&
    inner.y >= outer.y &&
    inner.x + inner.width <= outer.x + outer.width &&
    inner.y + inner.height <= outer.y + outer.height
  );
}
function intersects(a: WorkflowRectangle, b: WorkflowRectangle) {
  return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

test('ELK keeps custom failure endpoints outside the review loop and folds standard error routes', async () => {
  const reviews: GraphNode = {
    kind: 'par',
    name: 'reviews',
    join: { kind: 'all' },
    branches: [
      { kind: 'verifier', name: 'acceptance' },
      { kind: 'verifier', name: 'code' },
    ],
  };
  const result: GraphNode = {
    kind: 'choice',
    name: 'review_result',
    branches: [
      {
        when: { kind: 'any', guards: [error('acceptance'), error('code')] },
        node: fail('review_failed'),
      },
      {
        when: { kind: 'all', guards: [accepted('acceptance'), accepted('code')] },
        node: done('complete'),
      },
    ],
    otherwise: agent('review_repair'),
  };
  const choice: GraphNode = {
    kind: 'choice',
    name: 'worker_result',
    branches: [{ when: { ...error('worker'), labels: ['crash'] }, node: fail('worker_failed') }],
    otherwise: seq('changes', [
      loop('change_loop', seq('iteration', [reviews, result])),
      fail('exhausted'),
    ]),
  };
  const { projection, sizes, layout } = await arrange(seq('run', [agent('worker'), choice]));
  const retry = regionOf(projection, layout, 'change_loop');
  const parallel = regionOf(projection, layout, 'reviews');
  for (const outside of ['worker', 'worker_failed', 'exhausted']) {
    assert.equal(
      intersects(retry, nodeOf(projection, sizes, layout, outside)),
      false,
      `${outside} stays outside retry bounds`
    );
  }
  for (const inside of ['acceptance', 'code', 'complete', 'review_repair']) {
    assert.ok(
      contains(retry, nodeOf(projection, sizes, layout, inside)),
      `${inside} stays inside retry bounds`
    );
  }
  assert.ok(contains(retry, parallel), 'parallel review region fits inside retry region');
  assert.ok(contains(parallel, nodeOf(projection, sizes, layout, 'acceptance')));
  assert.ok(contains(parallel, nodeOf(projection, sizes, layout, 'code')));
  assert.equal(
    projection.nodes.some((node) => node.owner === 'review_failed'),
    false
  );
  const endpoint = projection.nodes.find((node) => node.owner === 'complete')!;
  assert.equal(endpoint.role, 'completion');
  assert.deepEqual(sizes[endpoint.id], { width: 132, height: 40 });
  assert.equal(intersects(parallel, nodeOf(projection, sizes, layout, 'complete')), false);
});

test('ELK nests equal-member regions with distinct bounds and absolute descendant coordinates', async () => {
  const choice: GraphNode = {
    kind: 'choice',
    name: 'worker_result',
    branches: [{ when: error('worker'), node: fail('worker_failed') }],
    otherwise: loop('outer_loop', loop('inner_loop', done('complete'))),
  };
  const { projection, sizes, layout } = await arrange(seq('run', [agent('worker'), choice]));
  const [innerRegion, outerRegion] = projection.regions;
  assert.deepEqual(innerRegion.nodeIds, outerRegion.nodeIds, 'compaction leaves equal member sets');
  const outer = regionOf(projection, layout, 'outer_loop');
  const inner = regionOf(projection, layout, 'inner_loop');
  assert.ok(contains(outer, inner), 'outer loop contains inner loop');
  assert.ok(
    outer.width > inner.width && outer.height > inner.height,
    'nested regions have distinct padding'
  );
  assert.ok(
    contains(inner, nodeOf(projection, sizes, layout, 'complete')),
    'terminal coordinate includes every ancestor offset'
  );
  assert.equal(
    projection.nodes.some((node) => node.owner === 'worker_failed'),
    false
  );
  assert.equal(
    projection.nodes.some((node) => node.owner === 'worker_result'),
    false
  );
  assert.equal(intersects(outer, nodeOf(projection, sizes, layout, 'worker')), false);
});

test('ELK separates sibling repeat regions and keeps a following activity outside both', async () => {
  const { projection, sizes, layout } = await arrange(
    seq('run', [
      loop('first_loop', agent('first_work')),
      loop('second_loop', agent('second_work')),
      agent('after_loops'),
    ])
  );
  const first = regionOf(projection, layout, 'first_loop');
  const second = regionOf(projection, layout, 'second_loop');
  assert.equal(intersects(first, second), false);
  assert.ok(contains(first, nodeOf(projection, sizes, layout, 'first_work')));
  assert.ok(contains(second, nodeOf(projection, sizes, layout, 'second_work')));
  const after = nodeOf(projection, sizes, layout, 'after_loops');
  assert.equal(intersects(first, after), false);
  assert.equal(intersects(second, after), false);
  assert.ok(after.x > second.x + second.width, 'default layout progresses to the right');
});
