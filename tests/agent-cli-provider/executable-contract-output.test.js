const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { test } = require('node:test');
const {
  assertNoSecret,
  runExecutable,
  runProviderExecutable,
} = require('./executable-contract-helpers.cjs');

function piFixture(name) {
  return fs.readFileSync(path.join(__dirname, '..', 'fixtures', 'pi', name), 'utf8');
}

function kiroFixture(name) {
  return fs.readFileSync(path.join(__dirname, '..', 'fixtures', 'kiro', name), 'utf8');
}

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

test('parse-output normalizes Pi JSON fixtures without duplicate snapshot text', () => {
  const textResponse = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'pi',
    stdout: piFixture('text.jsonl'),
  });

  assert.equal(textResponse.exitCode, 0);
  assert.equal(textResponse.envelope.ok, true);
  assert.deepEqual(textResponse.envelope.result.events, [
    { type: 'text', text: 'Hello from Pi' },
    {
      type: 'result',
      success: true,
      result: 'Hello from Pi',
      error: null,
      inputTokens: 7,
      outputTokens: 3,
      cacheReadInputTokens: 1,
      cacheCreationInputTokens: 0,
      cost: null,
      modelUsage: { input: 7, output: 3, cacheRead: 1, cacheWrite: 0 },
    },
  ]);

  const toolResponse = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'pi',
    stdout: piFixture('tool.jsonl'),
  });

  assert.equal(toolResponse.exitCode, 0);
  assert.equal(toolResponse.envelope.ok, true);
  assert.deepEqual(toolResponse.envelope.result.events, [
    { type: 'tool_call', toolName: 'bash', toolId: 'tool-1', input: { command: 'pwd' } },
    { type: 'tool_result', toolId: 'tool-1', content: 'line1', isError: false },
    { type: 'tool_result', toolId: 'tool-1', content: 'line1\n/tmp/pi', isError: false },
    { type: 'text', text: 'done' },
    {
      type: 'result',
      success: true,
      result: 'done',
      error: null,
      inputTokens: 9,
      outputTokens: 4,
      cacheReadInputTokens: 0,
      cacheCreationInputTokens: 0,
      cost: null,
      modelUsage: { input: 9, output: 4, cacheRead: 0, cacheWrite: 0 },
    },
  ]);
});

test('parse-output handles Pi failure and empty fixtures', () => {
  for (const [name, expectedError] of [
    ['command-failure.jsonl', 'Unknown option --bogus'],
    ['auth-failure.jsonl', 'authentication required: run /login'],
    ['rate-limit.jsonl', 'rate limit exceeded; retry later'],
    ['cancelled.jsonl', 'cancelled by user'],
  ]) {
    const response = runExecutable({
      schemaVersion: 1,
      command: 'parse-output',
      provider: 'pi',
      stdout: piFixture(name),
    });

    const lastEvent = response.envelope.result.events.at(-1);
    assert.equal(response.exitCode, 0);
    assert.equal(response.envelope.ok, true);
    assert.equal(lastEvent.type, 'result');
    assert.equal(lastEvent.success, false);
    assert.equal(lastEvent.error, expectedError);
  }

  const emptyResponse = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'pi',
    stdout: piFixture('empty.jsonl'),
  });

  assert.equal(emptyResponse.exitCode, 0);
  assert.equal(emptyResponse.envelope.ok, true);
  assert.deepEqual(emptyResponse.envelope.result.events, []);
});

test('parse-output normalizes Kiro ACP fixtures and diagnostics', () => {
  const textResponse = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'kiro',
    stdout: kiroFixture('text.jsonl'),
  });

  assert.equal(textResponse.exitCode, 0);
  assert.equal(textResponse.envelope.ok, true);
  assert.deepEqual(textResponse.envelope.result.events, [
    { type: 'text', text: 'Hello' },
    { type: 'text', text: ' from Kiro' },
    {
      type: 'result',
      success: true,
      result: 'Hello from Kiro',
      error: null,
      inputTokens: 7,
      outputTokens: 3,
      cacheReadInputTokens: 1,
      cacheCreationInputTokens: 0,
      cost: null,
      modelUsage: {
        inputTokens: 7,
        outputTokens: 3,
        cacheReadInputTokens: 1,
        cacheCreationInputTokens: 0,
      },
    },
  ]);

  const toolResponse = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'kiro',
    stdout: kiroFixture('tool.jsonl'),
  });

  assert.equal(toolResponse.exitCode, 0);
  assert.equal(toolResponse.envelope.ok, true);
  assert.deepEqual(toolResponse.envelope.result.events, [
    { type: 'tool_call', toolName: 'bash', toolId: 'tool-1', input: { command: 'pwd' } },
    { type: 'tool_result', toolId: 'tool-1', content: 'line1', isError: false },
    { type: 'tool_result', toolId: 'tool-1', content: 'line1\n/tmp/kiro', isError: false },
    { type: 'text', text: 'done' },
    {
      type: 'result',
      success: true,
      result: 'done',
      error: null,
      inputTokens: 9,
      outputTokens: 4,
      cacheReadInputTokens: 0,
      cacheCreationInputTokens: 0,
      cost: null,
      modelUsage: {
        inputTokens: 9,
        outputTokens: 4,
        cacheReadInputTokens: 0,
        cacheCreationInputTokens: 0,
      },
    },
  ]);

  for (const [name, expectedError] of [
    ['auth-failure.jsonl', 'authentication required: run kiro auth login'],
    ['cancelled.jsonl', 'cancelled by user'],
  ]) {
    const response = runExecutable({
      schemaVersion: 1,
      command: 'parse-output',
      provider: 'kiro',
      stdout: kiroFixture(name),
    });

    const lastEvent = response.envelope.result.events.at(-1);
    assert.equal(response.exitCode, 0);
    assert.equal(response.envelope.ok, true);
    assert.equal(lastEvent.type, 'result');
    assert.equal(lastEvent.success, false);
    assert.equal(lastEvent.error, expectedError);
  }

  const malformedResponse = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'kiro',
    stdout: kiroFixture('malformed.jsonl'),
  });

  assert.equal(malformedResponse.exitCode, 0);
  assert.equal(malformedResponse.envelope.ok, true);
  assert.deepEqual(malformedResponse.envelope.result.events, [{ type: 'text', text: 'ok' }]);
  assert.equal(malformedResponse.envelope.result.diagnostics[0].kind, 'parse-error');

  const emptyResponse = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'kiro',
    stdout: kiroFixture('empty.jsonl'),
  });

  assert.equal(emptyResponse.exitCode, 0);
  assert.equal(emptyResponse.envelope.ok, true);
  assert.deepEqual(emptyResponse.envelope.result.events, []);
});

test('parse-output handles spec-compliant ACP content blocks and thought chunks', () => {
  const response = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'kiro',
    stdout: kiroFixture('thought.jsonl'),
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.deepEqual(response.envelope.result.events, [
    { type: 'thinking', text: 'Need a plan' },
    { type: 'thinking', text: ' first' },
    { type: 'text', text: 'Done' },
    {
      type: 'result',
      success: true,
      result: 'Done',
      error: null,
      cost: null,
      modelUsage: null,
    },
  ]);
});

test('parse-output accumulates ACP delta chunks into the terminal result', () => {
  const stdout = [
    JSON.stringify({
      jsonrpc: '2.0',
      method: 'session/update',
      params: {
        sessionId: 'kiro-session-1',
        update: {
          sessionUpdate: 'agent_message_chunk',
          messageId: 'msg-delta-1',
          content: { type: 'text', text: 'Hello' },
        },
      },
    }),
    JSON.stringify({
      jsonrpc: '2.0',
      method: 'session/update',
      params: {
        sessionId: 'kiro-session-1',
        update: {
          sessionUpdate: 'agent_message_chunk',
          messageId: 'msg-delta-1',
          content: { type: 'text', text: ' world' },
        },
      },
    }),
    JSON.stringify({
      jsonrpc: '2.0',
      result: {
        stopReason: 'end_turn',
      },
    }),
  ].join('\n');

  const response = runExecutable({
    schemaVersion: 1,
    command: 'parse-output',
    provider: 'kiro',
    stdout,
  });

  assert.equal(response.exitCode, 0);
  assert.equal(response.envelope.ok, true);
  assert.deepEqual(response.envelope.result.events, [
    { type: 'text', text: 'Hello' },
    { type: 'text', text: ' world' },
    {
      type: 'result',
      success: true,
      result: 'Hello world',
      error: null,
      cost: null,
      modelUsage: null,
    },
  ]);
});

test('classify-error maps Pi auth, rate-limit, cancellation, and command failures', () => {
  for (const [message, expectedCategory, expectedRetryable] of [
    ['authentication required: run /login', 'auth', false],
    ['rate limit exceeded; retry later', 'rate-limit', true],
    ['cancelled by user', 'permanent', false],
    ['Unknown option --bogus', 'permanent', false],
  ]) {
    const response = runExecutable({
      schemaVersion: 1,
      command: 'classify-error',
      provider: 'pi',
      error: { message },
    });

    assert.equal(response.exitCode, 0);
    assert.equal(response.envelope.ok, true);
    assert.equal(response.envelope.result.category, expectedCategory);
    assert.equal(response.envelope.result.classification.retryable, expectedRetryable);
  }
});

test('classify-error maps Kiro auth, retryable, cancellation, and malformed ACP failures', () => {
  for (const [message, expectedCategory, expectedRetryable] of [
    ['authentication required: run kiro auth login', 'auth', false],
    ['rate limit exceeded; retry later', 'rate-limit', true],
    ['cancelled by user', 'permanent', false],
    ['malformed ACP response', 'schema', false],
  ]) {
    const response = runExecutable({
      schemaVersion: 1,
      command: 'classify-error',
      provider: 'kiro',
      error: { message },
    });

    assert.equal(response.exitCode, 0);
    assert.equal(response.envelope.ok, true);
    assert.equal(response.envelope.result.category, expectedCategory);
    assert.equal(response.envelope.result.classification.retryable, expectedRetryable);
  }
});
