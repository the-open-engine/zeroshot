/**
 * MiniMax Output Parser Tests
 *
 * Tests parsing of MiniMax CLI wrapper JSON events.
 */

const assert = require('assert');
const { parseChunk, parseEvent } = require('../src/providers/minimax/output-parser');

describe('MiniMax output parser', () => {
  describe('parseEvent', () => {
    it('parses text events', () => {
      const line = JSON.stringify({ type: 'text', text: 'Hello from MiniMax!' });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, { type: 'text', text: 'Hello from MiniMax!' });
    });

    it('parses thinking events', () => {
      const line = JSON.stringify({ type: 'thinking', text: 'Let me analyze...' });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, { type: 'thinking', text: 'Let me analyze...' });
    });

    it('parses tool_call events', () => {
      const line = JSON.stringify({
        type: 'tool_call',
        toolName: 'read_file',
        toolId: 'call_123',
        input: { path: '/tmp/test.txt' },
      });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, {
        type: 'tool_call',
        toolName: 'read_file',
        toolId: 'call_123',
        input: { path: '/tmp/test.txt' },
      });
    });

    it('parses tool_result events', () => {
      const line = JSON.stringify({
        type: 'tool_result',
        toolId: 'call_123',
        content: 'file contents here',
        isError: false,
      });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, {
        type: 'tool_result',
        toolId: 'call_123',
        content: 'file contents here',
        isError: false,
      });
    });

    it('parses result events', () => {
      const line = JSON.stringify({
        type: 'result',
        success: true,
        inputTokens: 100,
        outputTokens: 50,
      });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, {
        type: 'result',
        success: true,
        inputTokens: 100,
        outputTokens: 50,
        error: null,
      });
    });

    it('parses error events', () => {
      const line = JSON.stringify({
        type: 'error',
        error: 'API key invalid',
      });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, {
        type: 'result',
        success: false,
        error: 'API key invalid',
      });
    });

    it('returns null for invalid JSON', () => {
      assert.strictEqual(parseEvent('not json'), null);
    });

    it('returns null for unknown event types', () => {
      const line = JSON.stringify({ type: 'unknown_type', data: 'whatever' });
      assert.strictEqual(parseEvent(line), null);
    });

    it('returns null for empty text events', () => {
      const line = JSON.stringify({ type: 'text', text: '' });
      assert.strictEqual(parseEvent(line), null);
    });

    it('handles tool_call with name/id aliases', () => {
      const line = JSON.stringify({
        type: 'tool_call',
        name: 'write_file',
        id: 'call_456',
        input: { path: '/tmp/out.txt', content: 'hello' },
      });
      const result = parseEvent(line);
      assert.strictEqual(result.toolName, 'write_file');
      assert.strictEqual(result.toolId, 'call_456');
    });

    it('handles tool_result with isError true', () => {
      const line = JSON.stringify({
        type: 'tool_result',
        toolId: 'call_789',
        content: 'Permission denied',
        isError: true,
      });
      const result = parseEvent(line);
      assert.strictEqual(result.isError, true);
    });
  });

  describe('parseChunk', () => {
    it('parses a full stream with text and result', () => {
      const lines = [
        JSON.stringify({ type: 'text', text: 'Implementation complete.' }),
        JSON.stringify({ type: 'result', success: true, inputTokens: 200, outputTokens: 80 }),
      ];
      const events = parseChunk(lines.join('\n'));

      assert.strictEqual(events.length, 2);
      assert.deepStrictEqual(events[0], { type: 'text', text: 'Implementation complete.' });
      assert.deepStrictEqual(events[1], {
        type: 'result',
        success: true,
        inputTokens: 200,
        outputTokens: 80,
        error: null,
      });
    });

    it('skips empty lines and invalid JSON', () => {
      const chunk = '\n' + JSON.stringify({ type: 'text', text: 'hi' }) + '\nnot-json\n\n';
      const events = parseChunk(chunk);
      assert.strictEqual(events.length, 1);
      assert.strictEqual(events[0].text, 'hi');
    });

    it('handles error in stream', () => {
      const lines = [
        JSON.stringify({ type: 'error', error: 'Rate limit exceeded' }),
      ];
      const events = parseChunk(lines.join('\n'));

      assert.strictEqual(events.length, 1);
      assert.strictEqual(events[0].type, 'result');
      assert.strictEqual(events[0].success, false);
      assert.strictEqual(events[0].error, 'Rate limit exceeded');
    });
  });
});
