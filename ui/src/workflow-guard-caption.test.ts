import test from 'node:test';
import assert from 'node:assert/strict';
import type { Document } from './domain';
import { compactGuardLabel, guardLabel, projectWorkflow } from './workflow-projection';
import { compactWorkflow } from './workflow-display';

const error = (name: string) => ({
  kind: 'in',
  value: { name, source: 'error', field: null },
  labels: ['crash', 'malformed', 'refusal', 'timeout'],
});
const accepted = (name: string) => ({
  kind: 'in',
  value: { name, source: 'signal', field: 'verdict' },
  labels: ['accepted'],
});
function document(guards: unknown[]): Document {
  return {
    name: 'product-launch-content-kit',
    graph: {
      profile: 'openengine.graph.full/v1',
      initialInput: { kind: 'null' },
      policy: {},
      root: {
        kind: 'choice',
        name: 'content_outcome',
        branches: guards.map((when, i) => ({
          when,
          node: { kind: 'fail', name: `outcome_${i}`, reason: 'review_failed' },
        })),
        otherwise: { kind: 'succeed', name: 'done' },
      },
    },
    runtime: { harness: '', provider: '', size: 'small', nodes: {} },
  };
}

test('the five-worker launch outcome has a compact caption while preserving its condition and edge identity', () => {
  const guard = {
    kind: 'any',
    guards: ['website_copy', 'email_copy', 'social_copy', 'factual_review', 'brand_review'].map(
      error
    ),
  };
  const doc = document([guard]);
  const before = structuredClone(doc);
  const original = projectWorkflow(doc);
  const edge = original.edges.find((entry) => entry.branchIndex === 0)!;
  assert.equal(edge.shortLabel, '1. Any error');
  assert.equal(edge.label, `1. ${guardLabel(guard)}`);
  assert.ok(edge.label.endsWith('…'), 'the detailed caption still uses the existing bounded text');
  const displayed = compactWorkflow(original).edges.find((entry) => entry.id === edge.id)!;
  assert.equal(displayed.shortLabel, '1. Any error');
  assert.equal(displayed.fullLabel, edge.label);
  assert.equal(displayed.owner, 'content_outcome');
  assert.equal(displayed.branchIndex, 0);
  assert.equal(displayed.source, edge.source);
  assert.equal(displayed.target, edge.target);
  assert.deepEqual(doc, before, 'all five original selectors and error labels remain authored');
});

test('long names and many flat clauses cannot truncate Any error or All accepted recognition', () => {
  const names = Array.from(
    { length: 12 },
    (_, i) => `review_${i}_${'very_long_authored_name_'.repeat(12)}(with_parentheses)`
  );
  const guards = [
    { kind: 'any', guards: names.map(error) },
    { kind: 'all', guards: names.map(accepted) },
  ];
  const projected = projectWorkflow(document(guards));
  const displayed = compactWorkflow(projected);
  assert.deepEqual(
    displayed.edges.filter((edge) => edge.branchIndex !== undefined).map((edge) => edge.shortLabel),
    ['1. Any error', '2. All accepted']
  );
  for (const edge of projected.edges.filter((entry) => entry.branchIndex !== undefined))
    assert.equal(edge.label, `${edge.branchIndex! + 1}. ${guardLabel(guards[edge.branchIndex!])}`);
});

test('an incomplete or different fifth clause is never hidden by a compact caption', () => {
  const incomplete = {
    kind: 'any',
    guards: Array.from({ length: 5 }, (_, i) => error(`worker_${i}`)),
  };
  incomplete.guards[4].labels = ['crash'];
  const rejected = {
    kind: 'all',
    guards: Array.from({ length: 5 }, (_, i) => accepted(`review_${i}`)),
  };
  rejected.guards[4].labels = ['rejected'];
  for (const guard of [incomplete, rejected]) {
    assert.equal(compactGuardLabel(guard), undefined);
    const edge = compactWorkflow(projectWorkflow(document([guard]))).edges[0];
    assert.equal(edge.shortLabel, edge.label);
    assert.equal(edge.fullLabel, `1. ${guardLabel(guard)}`);
  }
});

test('unknown, malformed, mixed and nested guards retain their authored display text', () => {
  const cases: unknown[] = [
    { kind: 'any', guards: [error('a'), { kind: 'any', guards: [error('b'), error('c')] }] },
    { kind: 'all', guards: [accepted('a'), { kind: 'not', guard: accepted('b') }] },
    { kind: 'any', guards: [error('a'), accepted('b')] },
    { kind: 'all', guards: [error('a'), error('b')] },
    { kind: 'any', guards: [accepted('a'), accepted('b')] },
    {
      kind: 'any',
      guards: [error('a'), { ...error('b'), labels: ['crash', 'crash', 'malformed', 'timeout'] }],
    },
    {
      kind: 'any',
      guards: [error('a'), { ...error('b'), labels: [...error('b').labels, 'future_error'] }],
    },
    { kind: 'any', guards: [error('a'), { ...error('b'), extension: 'preserve' }] },
    { kind: 'any', guards: [error('a'), error('b')], extension: 'preserve' },
    {
      kind: 'any',
      guards: [
        error('a'),
        { ...error('b'), value: { name: 'b', source: 'error', field: 'future' } },
      ],
    },
    {
      kind: 'all',
      guards: [
        accepted('a'),
        { ...accepted('b'), value: { name: 'b', source: 'group', field: 'verdict' } },
      ],
    },
    {
      kind: 'all',
      guards: [
        accepted('a'),
        { ...accepted('b'), value: { name: 'b', source: 'signal', field: 'status' } },
      ],
    },
    { kind: 'any', guards: [error('a'), null] },
    { kind: 'any', guards: [] },
    { kind: 'future', guards: [error('a'), error('b')] },
    null,
  ];
  for (const guard of cases) {
    assert.equal(compactGuardLabel(guard), undefined, JSON.stringify(guard));
    const doc = document([guard]);
    const before = structuredClone(doc);
    const edge = compactWorkflow(projectWorkflow(doc)).edges[0];
    assert.equal(edge.shortLabel, edge.label, JSON.stringify(guard));
    assert.deepEqual(doc, before);
  }
});
