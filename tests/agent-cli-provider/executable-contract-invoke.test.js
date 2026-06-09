const assert = require('node:assert/strict');
const fs = require('node:fs');
const { test } = require('node:test');
const {
  assertNoSecret,
  fakeCodexScript,
  invokeCodexSchemaRequest,
  runExecutable,
  runProviderExecutable,
  runnerResult,
  withFakeProviderCli,
} = require('./executable-contract-helpers.cjs');

test('invoke returns redacted terminal evidence, parsed events, status, timing, and cleanup', async () => {
  let runnerCommand = null;
  const secret = 'super-secret-token';
  const response = await runProviderExecutable(
    invokeCodexSchemaRequest({
      env: {
        CUSTOM_TOKEN: secret,
      },
    }),
    {
      runner: (commandSpec) => {
        runnerCommand = commandSpec;
        assert.equal(fs.existsSync(commandSpec.cleanup[0]), true);
        return runnerResult({
          stdout: JSON.stringify({
            type: 'item.completed',
            item: {
              type: 'message',
              role: 'assistant',
              content: [{ type: 'text', text: `done ${secret}` }],
            },
          }),
          stderr: `warn ${secret}`,
          durationMs: 12,
        });
      },
    }
  );

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.result.exitCode, 0);
  assert.equal(response.envelope.result.signal, null);
  assert.equal(response.envelope.result.durationMs, 12);
  assert.equal(response.envelope.result.evidence.stdout.includes(secret), false);
  assert.equal(response.envelope.result.evidence.stderr.includes(secret), false);
  assert.equal(response.envelope.result.events[0].type, 'text');
  assert.equal(response.envelope.result.events[0].text.includes(secret), false);
  assert.equal(response.envelope.result.cleanup[0].removed, true);
  assert.equal(fs.existsSync(runnerCommand.cleanup[0]), false);
  assertNoSecret(response.envelope, secret);
});

test('invoke removes schema cleanup files when runner rejects', async () => {
  let cleanupPath = null;
  const response = await runProviderExecutable(invokeCodexSchemaRequest(), {
    runner: (commandSpec) => {
      cleanupPath = commandSpec.cleanup[0];
      assert.equal(fs.existsSync(cleanupPath), true);
      return Promise.reject(new Error('spawn ENOENT'));
    },
  });

  assert.equal(response.exitCode, 5);
  assert.equal(response.envelope.ok, false);
  assert.equal(response.envelope.error.code, 'internal-error');
  assert.equal(fs.existsSync(cleanupPath), false);
});

test('invoke redacts authEnv values from runner rejection envelopes', async () => {
  const secret = 'authenv-secret-123';
  const response = await runProviderExecutable(
    {
      schemaVersion: 1,
      command: 'invoke',
      provider: 'claude',
      context: 'hi',
      options: {
        authEnv: {
          CUSTOM: secret,
        },
      },
    },
    {
      runner: () => Promise.reject(new Error(`runner failed ${secret}`)),
    }
  );

  assert.equal(response.exitCode, 5);
  assert.equal(response.envelope.ok, false);
  assert.equal(response.envelope.error.code, 'internal-error');
  assert.equal(response.envelope.error.message.includes(secret), false);
  assertNoSecret(response.envelope, secret);
});

test('invoke exposes timeout evidence and cleanup when provider times out', async () => {
  let cleanupPath = null;
  const response = await runProviderExecutable(invokeCodexSchemaRequest({ timeoutMs: 50 }), {
    runner: (commandSpec) => {
      cleanupPath = commandSpec.cleanup[0];
      assert.equal(fs.existsSync(cleanupPath), true);
      return runnerResult({
        exitCode: null,
        signal: 'SIGKILL',
        durationMs: 151,
        timedOut: true,
        timeoutMs: 50,
        stderr: 'still running',
      });
    },
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.result.timedOut, true);
  assert.equal(response.envelope.result.timeoutMs, 50);
  assert.equal(response.envelope.result.exitCode, null);
  assert.equal(response.envelope.result.signal, 'SIGKILL');
  assert.equal(response.envelope.result.classification.retryable, true);
  assert.equal(response.envelope.evidence.timedOut, true);
  assert.equal(response.envelope.evidence.timeoutMs, 50);
  assert.equal(response.envelope.result.cleanup[0].removed, true);
  assert.equal(fs.existsSync(cleanupPath), false);
});

test('invoke redacts provider credentials inherited from process env', async () => {
  const secret = 'plain-provider-secret';
  const previous = process.env.OPENAI_API_KEY;
  process.env.OPENAI_API_KEY = secret;

  try {
    const response = await runProviderExecutable(
      {
        schemaVersion: 1,
        command: 'invoke',
        provider: 'codex',
        context: 'Return JSON.',
        options: {
          outputFormat: 'json',
          cliFeatures: {
            supportsJson: true,
            supportsSkipGitRepoCheck: true,
          },
        },
      },
      {
        runner: () =>
          runnerResult({
            stdout: `leak ${secret}`,
            stderr: `auth failed for ${secret}`,
          }),
      }
    );

    assert.equal(response.exitCode, 0);
    assert.equal(response.envelope.ok, true);
    assert.equal(response.envelope.result.evidence.stdout.includes(secret), false);
    assert.equal(response.envelope.result.evidence.stderr.includes(secret), false);
    assertNoSecret(response.envelope, secret);
  } finally {
    if (previous === undefined) {
      delete process.env.OPENAI_API_KEY;
    } else {
      process.env.OPENAI_API_KEY = previous;
    }
  }
});

test('invoke closes provider stdin and parses output from the spawned process', () => {
  withFakeProviderCli(
    'codex',
    fakeCodexScript(`
if (process.argv.includes('--help')) {
  process.stdout.write('Usage: codex exec --json --skip-git-repo-check\\n');
  process.exit(0);
}
process.stdin.setEncoding('utf8');
process.stdin.on('data', () => {});
process.stdin.on('end', () => {
  process.stdout.write(JSON.stringify({
    type: 'item.completed',
    item: {
      type: 'message',
      role: 'assistant',
      content: [{ type: 'text', text: 'HELPER_INVOKE_OK' }],
    },
  }));
});
process.stdin.resume();
`),
    () => {
      const response = runExecutable({
        schemaVersion: 1,
        command: 'invoke',
        provider: 'codex',
        context: 'Reply with exactly: HELPER_INVOKE_OK',
        options: {
          outputFormat: 'json',
        },
        timeoutMs: 300,
      });

      assert.equal(response.exitCode, 0);
      assert.equal(response.envelope.ok, true);
      assert.equal(response.envelope.result.timedOut, false);
      assert.equal(response.envelope.result.exitCode, 0);
      assert.equal(response.envelope.result.events[0].type, 'text');
      assert.equal(response.envelope.result.events[0].text, 'HELPER_INVOKE_OK');
      assert.ok(response.envelope.result.commandSpec.args.includes('--json'));
    }
  );
});
