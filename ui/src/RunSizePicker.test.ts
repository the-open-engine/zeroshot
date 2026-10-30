import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { RunSizePicker } from './RunSizePicker';
import type { MachineOption } from './machines';

const schema = { $defs: { RunSize: { enum: ['small', 'medium', 'large', 'xl', 'gpu_t4'] } } };
const large: MachineOption = {
  size: 'large',
  label: 'Large',
  vcpus: 4,
  memoryMiB: 16384,
  diskGiB: 128,
  laneUnits: 2,
};
const gpu: MachineOption = {
  size: 'gpu_t4',
  label: 'T4 GPU',
  vcpus: 4,
  memoryMiB: 16384,
  diskGiB: 64,
  laneUnits: 6,
  gpu: { name: 'NVIDIA T4', memoryMiB: 16384 },
};

function render(value: string, machines?: readonly MachineOption[]) {
  return renderToStaticMarkup(
    createElement(RunSizePicker, {
      value,
      machines,
      schema,
      onChange: () => assert.fail('rendering cannot edit the profile'),
    })
  );
}

test('host machine choices state size, lane weight and full Boost cost', () => {
  const html = render('large', [large, gpu]);
  assert.match(html, /Large · 2 lanes/);
  assert.doesNotMatch(html, /vCPU|GiB|VRAM/);
  assert.match(html, /Starting this size with Boost uses 2 Boosts for the run/);
  assert.doesNotMatch(html, /value="small"|value="medium"|value="xl"/);
  const gpuHtml = render('gpu_t4', [large, gpu]);
  assert.match(gpuHtml, /T4 GPU · 6 lanes/);
  assert.doesNotMatch(gpuHtml, /vCPU|GiB|VRAM/);
  assert.match(gpuHtml, /Starting this size with Boost uses 6 Boosts for the run/);
});

test('an unavailable saved size remains selected without replacing it', () => {
  const html = render('small', [large]);
  assert.match(html, /<option value="small"[^>]*disabled=""[^>]*selected=""/);
  assert.match(html, /small \(unavailable\)/);
  const empty = render('large', []);
  assert.match(empty, /large \(unavailable\)/);
  assert.doesNotMatch(empty, /4 vCPU|Boost uses/);
});

test('local schema fallback lists sizes without adding billing facts', () => {
  const html = render('small');
  for (const size of schema.$defs.RunSize.enum) assert.match(html, new RegExp(`value="${size}"`));
  assert.doesNotMatch(html, /Boost|lanes|unavailable/);
});

test('host metadata cannot offer sizes outside the native schema', () => {
  const machines = [large, { ...gpu, size: 'gpu' }];
  const html = render('large', machines);
  assert.match(html, /value="large"/);
  assert.doesNotMatch(html, /value="gpu"/);
  const saved = render('gpu', machines);
  assert.match(saved, /<option value="gpu"[^>]*disabled=""[^>]*selected=""/);
  assert.match(saved, /gpu \(unavailable\)/);
  assert.doesNotMatch(saved, /Starting this size with Boost/);
});
