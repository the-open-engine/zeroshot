/**
 * Codex helper parser tests.
 */

const assert = require('assert');
const { parseProviderChunk } = require('../src/providers');

describe('Codex output parser facade', () => {
  it('handles Codex-style agent messages', () => {
    const line = JSON.stringify({
      type: 'item.completed',
      item: {
        type: 'agent_message',
        id: 'item_0',
        text: '{"complexity":"TRIVIAL"}',
      },
    });

    assert.deepStrictEqual(parseProviderChunk('codex', line), [
      { type: 'text', text: '{"complexity":"TRIVIAL"}' },
    ]);
  });

  it('handles function call items', () => {
    const line = JSON.stringify({
      type: 'item.completed',
      item: {
        type: 'function_call',
        name: 'read_file',
        call_id: 'call_123',
        arguments: '{"path":"/tmp/test.txt"}',
      },
    });

    assert.deepStrictEqual(parseProviderChunk('codex', line), [
      {
        type: 'tool_call',
        toolName: 'read_file',
        toolId: 'call_123',
        input: { path: '/tmp/test.txt' },
      },
    ]);
  });

  it('ignores thread and turn start events', () => {
    const output = ['{"type":"thread.started","thread_id":"abc"}', '{"type":"turn.started"}'].join(
      '\n'
    );

    assert.deepStrictEqual(parseProviderChunk('codex', output), []);
  });

  it('parses turn.completed with usage stats', () => {
    const line = JSON.stringify({
      type: 'turn.completed',
      usage: { input_tokens: 100, output_tokens: 50 },
    });

    assert.deepStrictEqual(parseProviderChunk('codex', line), [
      {
        type: 'result',
        success: true,
        inputTokens: 100,
        outputTokens: 50,
      },
    ]);
  });

  it('parses complete Codex output streams', () => {
    const output = [
      JSON.stringify({ type: 'thread.started', thread_id: 'abc' }),
      JSON.stringify({ type: 'turn.started' }),
      JSON.stringify({
        type: 'item.completed',
        item: {
          id: 'item_0',
          type: 'agent_message',
          text: '{"complexity":"TRIVIAL","taskType":"INQUIRY"}',
        },
      }),
      JSON.stringify({
        type: 'turn.completed',
        usage: { input_tokens: 13202, output_tokens: 66 },
      }),
    ].join('\n');

    assert.deepStrictEqual(parseProviderChunk('codex', output), [
      {
        type: 'text',
        text: '{"complexity":"TRIVIAL","taskType":"INQUIRY"}',
      },
      {
        type: 'result',
        success: true,
        inputTokens: 13202,
        outputTokens: 66,
      },
    ]);
  });
});
