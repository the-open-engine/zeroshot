const assert = require('assert');

const { buildCompletionResult } = require('../src/agent/agent-task-executor');

function createAgent(options = {}) {
  return {
    id: options.id || 'validator-code',
    role: options.role || 'validator',
    processPid: null,
    config: {
      outputFormat: options.outputFormat ?? 'json',
      jsonSchema:
        options.jsonSchema === undefined
          ? {
              type: 'object',
              properties: {
                approved: { type: 'boolean' },
              },
              required: ['approved'],
            }
          : options.jsonSchema,
      cwd: options.cwd || process.cwd(),
    },
    worktree: null,
    isolation: null,
    _parseResultOutput:
      options.parseResultOutput ||
      (() => ({
        approved: true,
      })),
  };
}

function createState(output) {
  return {
    output,
    logFilePath: '/tmp/task.log',
  };
}

describe('buildCompletionResult', function () {
  it('keeps completed structured tasks successful when output parses', async function () {
    const agent = createAgent();

    const result = await buildCompletionResult({
      agent,
      taskId: 'task-1',
      providerName: 'claude',
      state: createState('{"type":"result","structured_output":{"approved":true}}'),
      stdout: 'Status: completed',
      success: true,
    });

    assert.strictEqual(result.success, true);
    assert.strictEqual(result.error, null);
  });

  it('downgrades completed structured tasks to failure when output is unparsable', async function () {
    const agent = createAgent({
      parseResultOutput: () =>
        Promise.reject(new Error('Agent validator-code output missing required JSON block')),
    });

    const result = await buildCompletionResult({
      agent,
      taskId: 'task-2',
      providerName: 'claude',
      state: createState('partial output only'),
      stdout: 'Status: completed',
      success: true,
    });

    assert.strictEqual(result.success, false);
    assert.match(result.error, /missing required JSON block/);
  });

  it('does not require structured parsing for text-output agents', async function () {
    let parseCalls = 0;
    const agent = createAgent({
      outputFormat: 'text',
      jsonSchema: null,
      parseResultOutput: () => {
        parseCalls += 1;
        return Promise.reject(new Error('should not be called'));
      },
    });

    const result = await buildCompletionResult({
      agent,
      taskId: 'task-3',
      providerName: 'claude',
      state: createState('plain text output'),
      stdout: 'Status: completed',
      success: true,
    });

    assert.strictEqual(result.success, true);
    assert.strictEqual(result.error, null);
    assert.strictEqual(parseCalls, 0);
  });
});
