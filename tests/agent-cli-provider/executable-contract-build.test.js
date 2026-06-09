const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {
  assertNoSecret,
  fakeCodexScript,
  runExecutable,
  withFakeProviderCli,
  withTempEnv,
} = require('./executable-contract-helpers.cjs');

test('build-command returns command spec without executing provider CLI', () => {
  const response = runExecutable({
    schemaVersion: 1,
    command: 'build-command',
    provider: 'codex',
    context: 'Return JSON.',
    options: {
      outputFormat: 'json',
      cwd: '/tmp/project',
      cliFeatures: {
        supportsJson: true,
        supportsCwd: true,
        supportsSkipGitRepoCheck: true,
      },
    },
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.stderr, '');
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.schemaVersion, 1);
  assert.equal(response.envelope.command, 'build-command');
  assert.equal(response.envelope.provider, 'codex');
  assert.equal(typeof response.envelope.adapterVersion, 'string');
  assert.equal(response.envelope.result.commandSpec.binary, 'codex');
  assert.equal(response.envelope.result.commandSpec.cwd, '/tmp/project');
  assert.ok(Array.isArray(response.envelope.result.commandSpec.args));
  assert.equal(typeof response.envelope.result.commandSpec.env, 'object');
  assert.ok(Array.isArray(response.envelope.warnings));
  assert.ok(Array.isArray(response.envelope.redactions));
});

test('build-command preserves Claude resume and continue options through JSON contract', () => {
  const resumed = runExecutable({
    schemaVersion: 1,
    command: 'build-command',
    provider: 'claude',
    context: 'ctx',
    options: {
      resumeSessionId: 'sess-1',
    },
  });

  assert.equal(resumed.exitCode, 0);
  assert.equal(resumed.envelope.ok, true);
  assert.deepEqual(resumed.envelope.result.commandSpec.args.slice(-3), [
    '--resume',
    'sess-1',
    'ctx',
  ]);

  const continued = runExecutable({
    schemaVersion: 1,
    command: 'build-command',
    provider: 'claude',
    context: 'ctx',
    options: {
      continueSession: true,
    },
  });

  assert.equal(continued.exitCode, 0);
  assert.equal(continued.envelope.ok, true);
  assert.deepEqual(continued.envelope.result.commandSpec.args.slice(-2), ['--continue', 'ctx']);
});

test('build-command redacts adapter auth env values from command spec output', () => {
  const secret = 'plain-secret';
  const response = runExecutable({
    schemaVersion: 1,
    command: 'build-command',
    provider: 'claude',
    context: 'Return JSON.',
    options: {
      authEnv: {
        CUSTOM: secret,
      },
    },
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.result.commandSpec.env.CUSTOM.includes(secret), false);
  assertNoSecret(response.envelope, secret);
});

test('build-command preserves metadata when benign env values match contract fields', () => {
  for (const { env, context, expectedProvider, expectedBinary, expectedAdapterVersion } of [
    {
      env: { FOO: 'codex' },
      context: 'codex',
      expectedProvider: 'codex',
      expectedBinary: 'codex',
      expectedAdapterVersion: '1',
    },
    {
      env: { FOO: '1' },
      context: '1',
      expectedProvider: 'codex',
      expectedBinary: 'codex',
      expectedAdapterVersion: '1',
    },
  ]) {
    const response = runExecutable({
      schemaVersion: 1,
      command: 'build-command',
      provider: 'codex',
      context,
      env,
    });

    assert.equal(response.exitCode, 0);
    assert.equal(response.envelope.ok, true);
    assert.equal(response.envelope.provider, expectedProvider);
    assert.equal(response.envelope.adapterVersion, expectedAdapterVersion);
    assert.equal(response.envelope.result.commandSpec.binary, expectedBinary);
    assert.equal(response.envelope.result.commandSpec.args.at(-1), context);
    assert.equal(response.envelope.result.commandSpec.env.FOO, '[REDACTED:FOO]');
    assert.deepEqual(response.envelope.redactions, [{ kind: 'env', key: 'FOO' }]);
  }
});

test('build-command preserves stable evidence when benign env values match schema metadata', () => {
  const response = runExecutable({
    schemaVersion: 1,
    command: 'build-command',
    provider: 'codex',
    context: 'Return JSON.',
    env: {
      FORMAT: 'json',
      MODE: 'none',
    },
    options: {
      outputFormat: 'json',
    },
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.evidence.outputFormat, 'json');
  assert.equal(response.envelope.evidence.schemaMode, 'none');
  assert.equal(response.envelope.result.outputFormat, 'json');
  assert.equal(response.envelope.result.schemaMode, 'none');
});

test('build-command probes local Codex CLI features without caller-supplied cliFeatures', () => {
  withFakeProviderCli(
    'codex',
    fakeCodexScript(`
if (process.argv.includes('--help')) {
  process.stdout.write('Usage: codex exec --json --skip-git-repo-check -m --config --cwd -C\\n');
  process.exit(0);
}
process.stdout.write('unexpected execution');
process.exit(17);
`),
    () => {
      const response = runExecutable({
        schemaVersion: 1,
        command: 'build-command',
        provider: 'codex',
        context: 'Return JSON.',
        options: {
          outputFormat: 'json',
        },
      });

      assert.equal(response.exitCode, 0);
      assert.equal(response.envelope.ok, true);
      assert.ok(response.envelope.result.commandSpec.args.includes('--json'));
    }
  );
});

test('build-command resolves Codex settings default level and model overrides', () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-provider-settings-'));
  const settingsFile = path.join(tempDir, 'settings.json');

  fs.writeFileSync(
    settingsFile,
    JSON.stringify({
      providerSettings: {
        codex: {
          defaultLevel: 'level3',
          levelOverrides: {
            level3: {
              model: 'gpt-5.5',
              reasoningEffort: 'xhigh',
            },
          },
        },
      },
    })
  );

  try {
    withFakeProviderCli(
      'codex',
      fakeCodexScript(`
if (process.argv.includes('--help')) {
  process.stdout.write('Usage: codex exec --json --skip-git-repo-check -m --config\\n');
  process.exit(0);
}
process.exit(17);
`),
      () =>
        withTempEnv({ ZEROSHOT_SETTINGS_FILE: settingsFile }, () => {
          const response = runExecutable({
            schemaVersion: 1,
            command: 'build-command',
            provider: 'codex',
            context: 'ctx',
            options: {
              outputFormat: 'json',
            },
          });

          const args = response.envelope.result.commandSpec.args;
          assert.equal(response.exitCode, 0);
          assert.equal(response.envelope.ok, true);
          assert.ok(args.includes('--json'));
          assert.deepEqual(args.slice(args.indexOf('-m'), args.indexOf('-m') + 2), [
            '-m',
            'gpt-5.5',
          ]);
          assert.deepEqual(args.slice(args.indexOf('--config'), args.indexOf('--config') + 2), [
            '--config',
            'model_reasoning_effort="xhigh"',
          ]);
        })
    );
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test('probe reports capabilities and credential presence without exposing values', () => {
  const response = runExecutable({
    schemaVersion: 1,
    command: 'probe',
    provider: 'claude',
    helpText:
      'claude --output-format stream-json --json-schema --dangerously-skip-permissions --include-partial-messages --verbose --model',
    env: {
      ANTHROPIC_API_KEY: 'sk-ant-secret',
    },
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.result.provider.id, 'claude');
  assert.equal(response.envelope.result.credentials[0].key, 'ANTHROPIC_API_KEY');
  assert.equal(response.envelope.result.credentials[0].present, true);
  assertNoSecret(response.envelope, 'sk-ant-secret');
});

test('probe reads live Codex help when helpText is not supplied', () => {
  withFakeProviderCli(
    'codex',
    fakeCodexScript(`
if (process.argv.includes('--help')) {
  process.stdout.write('Usage: codex exec --json --skip-git-repo-check\\n');
  process.exit(0);
}
process.exit(17);
`),
    () => {
      const response = runExecutable({
        schemaVersion: 1,
        command: 'probe',
        provider: 'codex',
      });

      assert.equal(response.exitCode, 0);
      assert.equal(response.envelope.ok, true);
      assert.equal(response.envelope.result.capabilities.supportsJson, true);
      assert.equal(response.envelope.result.capabilities.supportsOutputSchema, false);
      assert.equal(response.envelope.result.capabilities.unknown, false);
    }
  );
});
