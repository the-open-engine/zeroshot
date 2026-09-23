import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError, createApiClient } from './api';
import {
  appendHistory,
  createExampleRunHistory,
  createRunHistorySource,
  readRunDetail,
} from './run-history-source';
import type { HistoryEvent, HistoryPage } from './run-history';
import { HISTORY_PENDING_RETRY_MS } from './history-response';
import { runDetailFixture as detail } from './test-support';

test('the HTTP source maps only the pending history problem to a retry delay', () => {
  const base = new URL('https://example.test/ui/api/');
  const source = createRunHistorySource(createApiClient(base), base);
  assert.equal(
    source.retryDelay?.(new ApiError(503, 'history_pending', 'History is pending.')),
    HISTORY_PENDING_RETRY_MS
  );
  assert.equal(
    source.retryDelay?.(new ApiError(503, 'history_unavailable', 'History is unavailable.')),
    undefined
  );
  assert.equal(source.retryDelay?.(new Error('not an HTTP problem')), undefined);
});

test('HTTP history details require supported definition and projection versions before replay', async () => {
  const base = new URL('https://example.test/ui/api/');
  let response: unknown = detail();
  const source = createRunHistorySource(
    createApiClient(base, async () => Response.json(response)),
    base
  );
  assert.deepEqual(await source.detail('run/selected'), detail());
  for (const field of ['version', 'projectionVersion']) {
    for (const version of [0, 2, '1', null, undefined]) {
      response = { ...detail(), [field]: version };
      await assert.rejects(
        source.detail('run/selected'),
        (error: unknown) => error instanceof ApiError && error.code === 'incompatible_history'
      );
    }
  }
});

test('run definition identity must exactly match the selected run', async () => {
  const expected = detail();
  assert.equal(readRunDetail(expected, expected.runId), expected);
  for (const runId of ['other-run', '', undefined]) {
    const base = new URL('https://example.test/ui/api/');
    const source = createRunHistorySource(
      createApiClient(base, async () => Response.json({ ...expected, runId })),
      base
    );
    await assert.rejects(
      source.detail(expected.runId),
      (error: unknown) => error instanceof ApiError && error.code === 'run_identity_mismatch'
    );
  }
});

test('simulated histories use the same explicit definition version checks', async () => {
  const mount = new URL('https://example.test/ui/');
  for (const versions of [
    { version: 1, projectionVersion: 1 },
    { version: undefined, projectionVersion: 1 },
    { version: 1, projectionVersion: 2 },
  ]) {
    const source = createExampleRunHistory(mount, async () =>
      Response.json([{ detail: { ...detail(), ...versions }, events: [] }])
    );
    if (versions.version === 1 && versions.projectionVersion === 1)
      assert.equal((await source.detail('run/selected')).example, true);
    else
      await assert.rejects(
        source.detail('run/selected'),
        (error: unknown) => error instanceof ApiError && error.code === 'incompatible_history'
      );
  }
});

test('simulated run lists keep successful output only in detail', async () => {
  const output = { report: 'large retained result' };
  const run = {
    ...detail(),
    phase: 'finished',
    terminal: { status: 'succeeded' as const, output },
  };
  const source = createExampleRunHistory(
    new URL('https://example.test/ui/'),
    async () => Response.json([{ detail: run, events: [] }])
  );
  const [summary] = (await source.list()).runs;
  assert.deepEqual(summary.terminal, { status: 'succeeded' });
  assert.equal('graph' in summary, false);
  assert.deepEqual((await source.detail(run.runId)).terminal, { status: 'succeeded', output });
});

const record = (cursor: string): HistoryEvent => ({
  cursor,
  event: { kind: 'safe_log', execution: null, line: cursor },
});
const page = (...records: HistoryEvent[]): HistoryPage => ({
  events: records,
  nextCursor: records.at(-1)?.cursor ?? 'v2:0',
  headCursor: 'v2:12',
  complete: false,
});

test('retried and overlapping replay pages do not duplicate events or mutate the earlier prefix', () => {
  const previous = Array.from({ length: 9 }, (_, index) => record(`v2:${index + 1}`));
  const incoming = page(record('v2:9'), record('v2:10'), record('v2:10'), record('v2:11'));
  const next = appendHistory(previous, incoming);
  assert.deepEqual(
    next.map(({ cursor }) => cursor),
    Array.from({ length: 11 }, (_, index) => `v2:${index + 1}`)
  );
  assert.deepEqual(
    previous.map(({ cursor }) => cursor),
    Array.from({ length: 9 }, (_, index) => `v2:${index + 1}`)
  );
  assert.deepEqual(appendHistory(next, incoming), next);
});

test('empty caught-up page preserves retained output', () => {
  const previous = [record('v2:1')];
  assert.deepEqual(
    appendHistory(previous, { events: [], nextCursor: 'v2:1', headCursor: 'v2:1', complete: true }),
    previous
  );
});

test('replay cannot jump over missing events, reorder new events, or accept a conflicting retransmission', () => {
  const previous = [record('v2:1')];
  for (const incoming of [
    page(record('v2:3')),
    page(record('v2:2'), record('v2:1')),
    page({
      cursor: 'v2:1',
      event: { kind: 'terminal', result: { status: 'succeeded', output: null } },
    }),
  ]) {
    assert.throws(() => appendHistory(previous, incoming), /gap or inconsistent cursor/);
    assert.equal(previous.length, 1);
  }
  assert.throws(
    () => appendHistory([record('v2:8')], page(record('v2:9'))),
    /gap or inconsistent cursor/
  );
});

test('next, head and completion cursors must describe the exact retained prefix', () => {
  const previous = [record('v2:1')];
  const valid = {
    events: [record('v2:2')],
    nextCursor: 'v2:2',
    headCursor: 'v2:3',
    complete: false,
  };
  for (const invalid of [
    { nextCursor: 'v2:3' },
    { nextCursor: 'v2:1' },
    { headCursor: 'v2:1' },
    { complete: true },
    { headCursor: 'v2:2' },
    { events: [] },
  ]) {
    assert.throws(
      () => appendHistory(previous, { ...valid, ...invalid }),
      /gap or inconsistent cursor/
    );
  }
  assert.deepEqual(
    appendHistory(previous, { ...valid, headCursor: 'v2:2', complete: true }).map(
      ({ cursor }) => cursor
    ),
    ['v2:1', 'v2:2']
  );
  assert.deepEqual(
    appendHistory([], { events: [], nextCursor: 'v2:0', headCursor: 'v2:0', complete: true }),
    []
  );
});

test('cursor comparisons retain native integer precision and reject invalid cursor syntax', () => {
  const first = page(record('v2:1'));
  assert.equal(appendHistory([], { ...first, headCursor: 'v2:9223372036854775807' }).length, 1);
  for (const headCursor of [
    'v2:9223372036854775808',
    'v2:01',
    'v2:-1',
    'v2:1.0',
    'v2:1e3',
    'other:1',
  ]) {
    assert.throws(() => appendHistory([], { ...first, headCursor }), /gap or inconsistent cursor/);
  }
});
