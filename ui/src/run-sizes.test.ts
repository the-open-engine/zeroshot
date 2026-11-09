import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError, readBootstrap } from './api';
import { validSizeId, validRunSizeCatalog } from './run-sizes';

const catalog = {
  kind: 'openengine.run-sizes/v2',
  default: 'custom/GPU-2027',
  sizes: [{ id: 'custom/GPU-2027', description: 'Target-owned CPU, RAM, disk and GPU specs.' }],
};

test('bootstrap accepts arbitrary target IDs and their descriptions and default', () => {
  const bootstrap = {
    version: 1,
    workspace: { kind: 'target', id: 'target-workspace' },
    templates: [],
    runtimeSchema: {},
    runSizes: catalog,
  };
  assert.deepEqual(readBootstrap(bootstrap).runSizes, catalog);
  assert.ok(validRunSizeCatalog(undefined));
  assert.ok(validRunSizeCatalog({ kind: catalog.kind, sizes: [] }));
  assert.ok(validRunSizeCatalog({ kind: catalog.kind, default: null, sizes: [] }));
});

test('catalog metadata rejects duplicate IDs, invalid defaults and malformed descriptors', () => {
  for (const runSizes of [
    null,
    { ...catalog, kind: 'unknown' },
    { ...catalog, sizes: null },
    { ...catalog, sizes: [catalog.sizes[0], catalog.sizes[0]] },
    { ...catalog, default: 'not-offered' },
    { ...catalog, default: undefined },
    { ...catalog, sizes: [] },
    { ...catalog, sizes: [{ id: '', description: 'Resources' }] },
    { ...catalog, sizes: [{ id: catalog.default, description: '   ' }] },
    { ...catalog, sizes: [{ id: catalog.default, description: 'Resources\nwith controls' }] },
    { ...catalog, sizes: [{ id: catalog.default, description: 'x'.repeat(1025) }] },
  ]) {
    assert.equal(validRunSizeCatalog(runSizes), false, JSON.stringify(runSizes));
    assert.throws(
      () => readBootstrap({ version: 1, templates: [], runtimeSchema: {}, runSizes }),
      (cause: unknown) => cause instanceof ApiError && cause.code === 'invalid_bootstrap'
    );
  }
});

test('size IDs use Unicode character bounds without semantic normalization', () => {
  for (const id of [
    'medium',
    'GPU/2027',
    'tiny',
    'standard',
    'a box',
    'é'.repeat(65),
    'é'.repeat(128),
    '🚀'.repeat(128),
  ])
    assert.equal(validSizeId(id), true);
  for (const id of [
    '',
    'é'.repeat(129),
    '🚀'.repeat(129),
    'x'.repeat(129),
    'bad\nsize',
    'bad\u0085size',
    null,
  ])
    assert.equal(validSizeId(id), false);
});

test('size descriptions retain an explicit UTF-8 byte bound', () => {
  for (const [description, valid] of [
    ['é'.repeat(512), true],
    ['é'.repeat(513), false],
  ] as const)
    assert.equal(
      validRunSizeCatalog({
        ...catalog,
        sizes: [{ id: catalog.default, description }],
      }),
      valid
    );
});
