import test from 'node:test';
import assert from 'node:assert/strict';
import type { Document } from './domain';
import { missingRuntimeSetting } from './runtime-readiness';

const document = (runtime: Partial<Document['runtime']>): Document => ({
  name: 'test',
  graph: {
    profile: 'openengine.graph.full/v1',
    initialInput: { kind: 'null' },
    policy: {},
    root: {
      kind: 'seq',
      name: 'run',
      children: [
        { kind: 'step', name: 'worker.a' },
        { kind: 'verifier', name: 'deliver' },
      ],
    },
  } as Document['graph'],
  runtime: {
    harness: 'codex',
    provider: 'openai',
    size: 'small',
    nodes: {
      'worker.a': { kind: 'agent', model: 'opaque-model' },
      deliver: { kind: 'git_delivery', connections: { github: ['GH_TOKEN'] } },
    },
    ...runtime,
  },
});

test('empty runtime choices are reported in Run settings order', () => {
  assert.deepEqual(missingRuntimeSetting(document({ harness: '', provider: '' })), {
    field: 'runtime.harness',
    message: 'Choose a harness.',
  });
  assert.deepEqual(missingRuntimeSetting(document({ provider: '' })), {
    field: 'runtime.provider',
    message: 'Choose a provider.',
  });
  assert.deepEqual(
    missingRuntimeSetting(document({ nodes: { 'worker.a': { kind: 'agent', model: '' } } })),
    { field: 'runtime.nodes.worker.a.model', message: 'Choose a model for `worker.a`.' }
  );
});

test('a complete runtime leaves validation to the server', () => {
  assert.equal(missingRuntimeSetting(document({})), undefined);
  assert.equal(missingRuntimeSetting(document({ harness: 'not-checked-here' })), undefined);
});
