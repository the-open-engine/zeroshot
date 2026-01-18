/**
 * Opencode Output Parser Tests
 *
 * Tests parsing of opencode --format json events.
 */

const assert = require('assert');
const { parseChunk, parseEvent, parsePart } = require('../src/providers/opencode/output-parser');

describe('Opencode output parser', () => {
  describe('parsePart', () => {
    it('handles text parts', () => {
      const part = { type: 'text', text: 'Hello!' };
      const result = parsePart(part);
      assert.deepStrictEqual(result, { type: 'text', text: 'Hello!' });
    });

    it('handles reasoning parts', () => {
      const part = { type: 'reasoning', text: 'Thinking...' };
      const result = parsePart(part);
      assert.deepStrictEqual(result, { type: 'thinking', text: 'Thinking...' });
    });

    it('handles tool parts (pending)', () => {
      const part = {
        type: 'tool',
        callID: 'call_123',
        tool: 'read_file',
        state: { status: 'pending', input: { path: '/tmp/test.txt' } },
      };
      const result = parsePart(part);
      assert.deepStrictEqual(result, {
        type: 'tool_call',
        toolName: 'read_file',
        toolId: 'call_123',
        input: { path: '/tmp/test.txt' },
      });
    });

    it('handles tool parts (completed)', () => {
      const part = {
        type: 'tool',
        callID: 'call_456',
        tool: 'read_file',
        state: { status: 'completed', input: {}, output: 'done' },
      };
      const result = parsePart(part);
      assert.deepStrictEqual(result, {
        type: 'tool_result',
        toolId: 'call_456',
        content: 'done',
        isError: false,
      });
    });

    it('handles step-finish parts', () => {
      const part = { type: 'step-finish', tokens: { input: 10, output: 4 } };
      const result = parsePart(part);
      assert.deepStrictEqual(result, {
        type: 'result',
        success: true,
        inputTokens: 10,
        outputTokens: 4,
      });
    });
  });

  describe('parseEvent', () => {
    it('parses top-level text events', () => {
      const line = JSON.stringify({ type: 'text', part: { type: 'text', text: 'Hi' } });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, { type: 'text', text: 'Hi' });
    });

    it('parses message.part.updated reasoning events', () => {
      const line = JSON.stringify({
        type: 'message.part.updated',
        properties: { part: { type: 'reasoning', text: 'Plan...' } },
      });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, { type: 'thinking', text: 'Plan...' });
    });

    it('parses error events', () => {
      const line = JSON.stringify({
        type: 'error',
        error: { name: 'ProviderAuthError', data: { message: 'Missing auth' } },
      });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, { type: 'result', success: false, error: 'Missing auth' });
    });
  });

  describe('parseChunk', () => {
    it('parses full stream with result', () => {
      const lines = [
        JSON.stringify({ type: 'step_start', part: { type: 'step-start' } }),
        JSON.stringify({ type: 'text', part: { type: 'text', text: 'Hello!' } }),
        JSON.stringify({
          type: 'step_finish',
          part: { type: 'step-finish', tokens: { input: 2, output: 1 } },
        }),
      ];
      const events = parseChunk(lines.join('\n'));

      assert.strictEqual(events.length, 2);
      assert.deepStrictEqual(events[0], { type: 'text', text: 'Hello!' });
      assert.deepStrictEqual(events[1], {
        type: 'result',
        success: true,
        inputTokens: 2,
        outputTokens: 1,
      });
    });
  });
});
