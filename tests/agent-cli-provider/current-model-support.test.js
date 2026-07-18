const assert = require('node:assert/strict');
const { test } = require('node:test');

const helper = require('../../lib/agent-cli-provider');
const { runExecutable } = require('./executable-contract-helpers.cjs');

const OPENAI_GPT_56_MODELS = ['gpt-5.6', 'gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna'];

const CURRENT_CLAUDE_MODELS = [
  'fable',
  'claude-fable-5',
  'claude-opus-4-8',
  'claude-opus-4-7',
  'claude-opus-4-6',
  'claude-opus-4-5',
  'claude-opus-4-5-20251101',
  'claude-sonnet-5',
  'claude-sonnet-4-6',
  'claude-sonnet-4-5',
  'claude-sonnet-4-5-20250929',
  'claude-haiku-4-5',
  'claude-haiku-4-5-20251001',
  'claude-mythos-5',
  'claude-mythos-preview',
];

test('Codex catalog accepts the GPT-5.6 family and alias', () => {
  const adapter = helper.getProviderAdapter('codex');

  for (const model of OPENAI_GPT_56_MODELS) {
    assert.equal(adapter.validateModelId(model), model);
  }
});

test('Claude catalog accepts current canonical ids, aliases, and limited-access models', () => {
  const adapter = helper.getProviderAdapter('claude');

  for (const model of CURRENT_CLAUDE_MODELS) {
    assert.equal(adapter.validateModelId(model), model);
  }
});

test('Codex sends max reasoning effort through its config override', () => {
  const spec = helper.buildProviderCommand('codex', 'test context', {
    modelSpec: { model: 'gpt-5.6-sol', reasoningEffort: 'max' },
    cliFeatures: {
      supportsConfigOverride: true,
      supportsSkipGitRepoCheck: true,
    },
  });

  assert.deepEqual(spec.args.slice(spec.args.indexOf('-m'), spec.args.indexOf('-m') + 2), [
    '-m',
    'gpt-5.6-sol',
  ]);
  assert.deepEqual(
    spec.args.slice(spec.args.indexOf('--config'), spec.args.indexOf('--config') + 2),
    ['--config', 'model_reasoning_effort="max"']
  );
});

test('Claude sends max reasoning effort through the installed CLI effort flag', () => {
  const spec = helper.buildProviderCommand('claude', 'test context', {
    modelSpec: { model: 'claude-fable-5', reasoningEffort: 'max' },
    cliFeatures: {
      supportsModel: true,
      supportsEffort: true,
    },
  });

  assert.deepEqual(
    spec.args.slice(spec.args.indexOf('--model'), spec.args.indexOf('--model') + 2),
    ['--model', 'claude-fable-5']
  );
  assert.deepEqual(
    spec.args.slice(spec.args.indexOf('--effort'), spec.args.indexOf('--effort') + 2),
    ['--effort', 'max']
  );
});

test('Claude registry reports reasoning-effort support', () => {
  const metadata = helper.getProviderRegistryEntry('claude');
  assert.equal(metadata.capabilities.reasoningEffort, true);
});

test('provider executable contract accepts and preserves max reasoning effort', () => {
  for (const { provider, model, flag, expected } of [
    {
      provider: 'codex',
      model: 'gpt-5.6-terra',
      flag: '--config',
      expected: 'model_reasoning_effort="max"',
    },
    {
      provider: 'claude',
      model: 'claude-sonnet-5',
      flag: '--effort',
      expected: 'max',
    },
  ]) {
    const response = runExecutable({
      schemaVersion: 1,
      command: 'build-command',
      provider,
      context: 'test context',
      options: {
        modelSpec: { model, reasoningEffort: 'max' },
      },
    });

    assert.equal(response.exitCode, 0, provider);
    assert.equal(response.envelope.ok, true, provider);
    const args = response.envelope.result.commandSpec.args;
    assert.deepEqual(args.slice(args.indexOf(flag), args.indexOf(flag) + 2), [flag, expected]);
  }
});

test('invalid-model diagnostics enumerate the current provider catalog', () => {
  for (const { provider, expectedModel } of [
    { provider: 'codex', expectedModel: 'gpt-5.6-sol' },
    { provider: 'claude', expectedModel: 'claude-opus-4-8' },
  ]) {
    let modelError;
    try {
      helper.getProviderAdapter(provider).validateModelId('not-a-current-model');
    } catch (error) {
      modelError = error;
    }
    assert.ok(modelError instanceof Error, provider);
    assert.ok(modelError.message.includes('Valid models:'), provider);
    assert.ok(modelError.message.includes(expectedModel), provider);
  }
});
