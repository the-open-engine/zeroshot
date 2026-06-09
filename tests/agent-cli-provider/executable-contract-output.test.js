const assert = require('node:assert/strict');
const { test } = require('node:test');
const {
  assertNoSecret,
  runExecutable,
  runProviderExecutable,
} = require('./executable-contract-helpers.cjs');

test('parse-output returns normalized events and parser diagnostics', () => {
  const stdout = [
    JSON.stringify({
      type: 'item.completed',
      item: {
        type: 'message',
        role: 'assistant',
        content: [{ type: 'text', text: 'hello' }],
      },
    }),
    '{bad json',
  ].join('\n');

  const response = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'codex',
    stdout,
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.deepEqual(response.envelope.result.events, [{ type: 'text', text: 'hello' }]);
  assert.equal(response.envelope.result.diagnostics[0].kind, 'parse-error');
});

test('parse-output redacts uppercase secret-key fields from provider events', () => {
  const secret = 'UPPERCASESECRET123';
  const response = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'claude',
    stdout: JSON.stringify({
      type: 'assistant',
      message: {
        content: [
          {
            type: 'tool_use',
            name: 'x',
            id: '1',
            input: {
              apiKey: secret,
              credential: {
                key: secret,
                present: true,
              },
            },
          },
        ],
      },
    }),
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.result.events[0].type, 'tool_call');
  assert.match(response.envelope.result.events[0].input.apiKey, /^\[REDACTED:/);
  assert.match(response.envelope.result.events[0].input.credential.key, /^\[REDACTED:/);
  assertNoSecret(response.envelope, secret);
});

test('parse-output redacts metadata-shaped provider event key fields', () => {
  const secret = 'METADATASHAPEDSECRET123';
  const response = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'claude',
    stdout: JSON.stringify({
      type: 'assistant',
      message: {
        content: [
          {
            type: 'tool_use',
            name: 'x',
            id: '1',
            input: {
              kind: 'env',
              key: secret,
            },
          },
        ],
      },
    }),
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.result.events[0].type, 'tool_call');
  assert.match(response.envelope.result.events[0].input.key, /^\[REDACTED:/);
  assertNoSecret(response.envelope, secret);
});

test('classify-error returns machine-readable category and redacted evidence', () => {
  const response = runExecutable({
    schemaVersion: 1,
    command: 'classify-error',
    provider: 'codex',
    error: {
      message: 'invalid_api_key: token sk-secret is revoked',
      status: 401,
    },
    env: {
      OPENAI_API_KEY: 'sk-secret',
    },
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.result.classification.retryable, false);
  assert.equal(response.envelope.result.category, 'auth');
  assertNoSecret(response.envelope, 'sk-secret');
});

test('classify-error redacts secret-looking evidence without matching env value', () => {
  const secret = 'sk-live-123456';
  const response = runExecutable({
    schemaVersion: 1,
    command: 'classify-error',
    provider: 'codex',
    error: {
      message: `invalid_api_key token ${secret} leaked`,
      status: 401,
    },
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.result.category, 'auth');
  assertNoSecret(response.envelope, secret);
});

test('classify-error uses status-only auth categories before adapter retryability', () => {
  for (const status of [401, 403]) {
    const response = runExecutable({
      schemaVersion: 1,
      command: 'classify-error',
      provider: 'codex',
      error: {
        message: 'Request failed',
        status,
      },
    });

    assert.equal(response.exitCode, 0);
    assert.equal(response.envelope.ok, true);
    assert.equal(response.envelope.result.category, 'auth');
    assert.equal(response.envelope.result.evidence.status, status);
  }
});

test('classify-error uses status-only rate-limit category before adapter retryability', () => {
  const response = runExecutable({
    schemaVersion: 1,
    command: 'classify-error',
    provider: 'codex',
    error: {
      message: 'Request failed',
      status: 429,
    },
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.equal(response.envelope.result.category, 'rate-limit');
  assert.equal(response.envelope.result.evidence.status, 429);
});

test('classify-error uses nested response status categories and evidence', () => {
  for (const { responseStatus, expectedCategory } of [
    { responseStatus: { status: 401 }, expectedCategory: 'auth' },
    { responseStatus: { statusCode: 403 }, expectedCategory: 'auth' },
    { responseStatus: { status: 429 }, expectedCategory: 'rate-limit' },
  ]) {
    const response = runExecutable({
      schemaVersion: 1,
      command: 'classify-error',
      provider: 'codex',
      error: {
        message: 'Request failed',
        response: responseStatus,
      },
    });

    const status = responseStatus.status ?? responseStatus.statusCode;
    assert.equal(response.exitCode, 0);
    assert.equal(response.envelope.ok, true);
    assert.equal(response.envelope.result.category, expectedCategory);
    assert.equal(response.envelope.result.evidence.status, status);
  }
});

test('parse-output reports parser-error diagnostics when provider parser throws', async () => {
  const adapters = require('../../lib/agent-cli-provider/adapters');
  const originalParseProviderChunk = adapters.parseProviderChunk;

  adapters.parseProviderChunk = () => {
    throw new Error('adapter parser crashed');
  };

  try {
    const response = await runProviderExecutable({
      schemaVersion: 1,
      command: 'parse-output',
      provider: 'codex',
      stdout: JSON.stringify({ type: 'turn.completed' }),
    });

    assert.equal(response.exitCode, 0);
    assert.equal(response.envelope.ok, true);
    assert.equal(response.envelope.result.events.length, 0);
    assert.equal(response.envelope.result.diagnostics[0].kind, 'parser-error');
    assert.match(response.envelope.result.diagnostics[0].message, /adapter parser crashed/);
  } finally {
    adapters.parseProviderChunk = originalParseProviderChunk;
  }
});
