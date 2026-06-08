/**
 * Opencode helper parser tests.
 */

const assert = require('assert');
const { parseProviderChunk } = require('../src/providers');

describe('Opencode output parser facade', () => {
  it('handles text parts', () => {
    const line = JSON.stringify({ type: 'text', part: { type: 'text', text: 'Hello!' } });
    assert.deepStrictEqual(parseProviderChunk('opencode', line), [
      { type: 'text', text: 'Hello!' },
    ]);
  });

  it('handles reasoning parts', () => {
    const line = JSON.stringify({
      type: 'message.part.updated',
      properties: { part: { type: 'reasoning', text: 'Thinking...' } },
    });
    assert.deepStrictEqual(parseProviderChunk('opencode', line), [
      { type: 'thinking', text: 'Thinking...' },
    ]);
  });

  it('handles pending tool parts', () => {
    const line = JSON.stringify({
      type: 'message.part.updated',
      properties: {
        part: {
          type: 'tool',
          callID: 'call_123',
          tool: 'read_file',
          state: { status: 'pending', input: { path: '/tmp/test.txt' } },
        },
      },
    });
    assert.deepStrictEqual(parseProviderChunk('opencode', line), [
      {
        type: 'tool_call',
        toolName: 'read_file',
        toolId: 'call_123',
        input: { path: '/tmp/test.txt' },
      },
    ]);
  });

  it('handles completed tool parts', () => {
    const line = JSON.stringify({
      type: 'message.part.updated',
      properties: {
        part: {
          type: 'tool',
          callID: 'call_456',
          tool: 'read_file',
          state: { status: 'completed', input: {}, output: 'done' },
        },
      },
    });
    assert.deepStrictEqual(parseProviderChunk('opencode', line), [
      {
        type: 'tool_result',
        toolId: 'call_456',
        content: 'done',
        isError: false,
      },
    ]);
  });

  it('handles step-finish parts', () => {
    const line = JSON.stringify({
      type: 'step_finish',
      part: { type: 'step-finish', tokens: { input: 10, output: 4 } },
    });
    assert.deepStrictEqual(parseProviderChunk('opencode', line), [
      {
        type: 'result',
        success: true,
        inputTokens: 10,
        outputTokens: 4,
      },
    ]);
  });

  it('parses error events', () => {
    const line = JSON.stringify({
      type: 'error',
      error: { name: 'ProviderAuthError', data: { message: 'Missing auth' } },
    });
    assert.deepStrictEqual(parseProviderChunk('opencode', line), [
      { type: 'result', success: false, error: 'Missing auth' },
    ]);
  });
});
