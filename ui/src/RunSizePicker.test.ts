import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { RunSizePicker } from './RunSizePicker';
import type { MachineOption } from './machines';
import type { RunSizeCatalog } from './run-sizes';

const large: MachineOption = {
  size: 'large',
  label: 'Large',
  vcpus: 4,
  memoryMiB: 16384,
  diskGiB: 128,
  laneUnits: 2,
};
const gpu: MachineOption = {
  size: 'future-gpu/2027',
  label: 'Future GPU',
  vcpus: 4,
  memoryMiB: 16384,
  diskGiB: 64,
  laneUnits: 6,
  gpu: { name: 'NVIDIA T4', memoryMiB: 16384 },
};
const catalog: RunSizeCatalog = {
  kind: 'openengine.run-sizes/v2',
  default: 'large',
  sizes: [
    { id: 'large', description: '4 vCPU and 16 GiB RAM.' },
    { id: 'future-gpu/2027', description: 'Target-provided GPU resources.' },
  ],
};

function render(value?: string, machines?: readonly MachineOption[], runSizes?: RunSizeCatalog) {
  return renderToStaticMarkup(
    createElement(RunSizePicker, {
      value,
      machines,
      runSizes,
      onChange: () => assert.fail('rendering cannot edit the profile'),
    })
  );
}

test('host machine choices annotate arbitrary target size IDs with lane and Boost cost', () => {
  const html = render('large', [large, gpu], catalog);
  assert.match(html, /Large · 2 lanes/);
  assert.doesNotMatch(html, /vCPU|GiB|VRAM/);
  assert.match(html, /Starting this size with Boost uses 2 Boosts for the run/);
  const gpuHtml = render('future-gpu/2027', [large, gpu], catalog);
  assert.match(gpuHtml, /Future GPU · 6 lanes/);
  assert.match(gpuHtml, /Starting this size with Boost uses 6 Boosts for the run/);
});

test('unavailable authored sizes remain selected without replacing the runtime', () => {
  const html = render('removed-big-box', [large], catalog);
  assert.match(html, /<option value="removed-big-box"[^>]*disabled=""[^>]*selected=""/);
  assert.match(html, /removed-big-box \(unavailable\)/);
  const empty = render('large', [], { kind: 'openengine.run-sizes/v2', sizes: [] });
  assert.match(empty, /large \(unavailable\)/);
  assert.match(empty, /No sizes available/);
  assert.doesNotMatch(empty, /4 vCPU|Boost uses/);
});

test('target catalog supplies choices without a global schema or billing facts', () => {
  const html = render('future-gpu/2027', undefined, catalog);
  for (const { id } of catalog.sizes) assert.ok(html.includes(`value="${id}"`));
  assert.match(html, /Target default \(large\)/);
  assert.doesNotMatch(html, /Boost|lanes|unavailable/);
});

test('target catalog controls offered IDs independently of machine presentation metadata', () => {
  const html = render('large', [large, { ...gpu, size: 'not-offered' }], catalog);
  assert.match(html, /value="future-gpu\/2027"/);
  assert.doesNotMatch(html, /value="not-offered"/);
  const saved = render('not-offered', [gpu], catalog);
  assert.match(saved, /not-offered \(unavailable\)/);
  assert.doesNotMatch(saved, /Starting this size with Boost/);
});

test('omitted size stays the target default without choosing an explicit size', () => {
  const html = render(undefined, undefined, catalog);
  assert.match(html, /<option value="" selected="">Target default \(large\)/);
  assert.doesNotMatch(html, /<option value="large" selected/);
});

test('without host catalogs size is an opaque freeform input, with no global fallback', () => {
  const html = render('custom/GPU-42');
  assert.match(html, /<input[^>]*value="custom\/GPU-42"/);
  assert.doesNotMatch(html, /<select|<option|unavailable|Boost/);
  const omitted = render();
  assert.match(omitted, /placeholder="Target default"/);
  assert.match(omitted, /value=""/);
});
