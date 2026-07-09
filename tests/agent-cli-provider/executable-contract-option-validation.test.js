const assert = require('node:assert/strict');
const { test } = require('node:test');
const {
  runExecutable,
  runProviderExecutable,
  runnerResult,
} = require('./executable-contract-helpers.cjs');

const invalidNestedOptionCases = [
  {
    name: 'cwd type',
    options: { cwd: 123, cliFeatures: { supportsCwd: true } },
    field: 'options.cwd',
  },
  { name: 'outputFormat enum', options: { outputFormat: 'xml' }, field: 'options.outputFormat' },
  { name: 'autoApprove type', options: { autoApprove: 'yes' }, field: 'options.autoApprove' },
  {
    name: 'resumeSessionId type',
    options: { resumeSessionId: 123 },
    field: 'options.resumeSessionId',
  },
  {
    name: 'continueSession type',
    options: { continueSession: 'yes' },
    field: 'options.continueSession',
  },
  { name: 'strictSchema type', options: { strictSchema: 'yes' }, field: 'options.strictSchema' },
  {
    name: 'cliFeatures boolean',
    options: { cliFeatures: { supportsCwd: 'true' } },
    field: 'options.cliFeatures.supportsCwd',
  },
  {
    name: 'cliFeatures supportsDir boolean',
    options: { cliFeatures: { supportsDir: 'true' } },
    field: 'options.cliFeatures.supportsDir',
  },
  { name: 'modelSpec object', options: { modelSpec: 'level2' }, field: 'options.modelSpec' },
  {
    name: 'modelSpec level enum',
    options: { modelSpec: { level: 'level4' } },
    field: 'options.modelSpec.level',
  },
  {
    name: 'modelSpec model type',
    options: { modelSpec: { model: 123 } },
    field: 'options.modelSpec.model',
  },
  {
    name: 'modelSpec reasoningEffort enum',
    options: { modelSpec: { reasoningEffort: 'extreme' } },
    field: 'options.modelSpec.reasoningEffort',
  },
];

test('build-command rejects invalid nested options before command spec creation', () => {
  for (const { name, options, field } of invalidNestedOptionCases) {
    const response = runExecutable({
      schemaVersion: 1,
      command: 'build-command',
      provider: 'codex',
      context: 'hi',
      options,
    });

    assert.equal(response.exitCode, 2, name);
    assert.equal(response.envelope.ok, false, name);
    assert.equal(response.envelope.error.code, 'invalid-field', name);
    assert.equal(response.envelope.error.field, field, name);
  }
});

test('invoke rejects invalid nested options before runner execution', async () => {
  for (const { name, options, field } of invalidNestedOptionCases) {
    let runnerCalled = false;
    const response = await runProviderExecutable(
      {
        schemaVersion: 1,
        command: 'invoke',
        provider: 'codex',
        context: 'hi',
        options,
      },
      {
        runner: () => {
          runnerCalled = true;
          return runnerResult();
        },
      }
    );

    assert.equal(response.exitCode, 2, name);
    assert.equal(response.envelope.ok, false, name);
    assert.equal(response.envelope.error.code, 'invalid-field', name);
    assert.equal(response.envelope.error.field, field, name);
    assert.equal(runnerCalled, false, name);
  }
});
