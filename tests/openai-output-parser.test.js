/**
 * Codex Output Parser Tests
 *
 * Tests for parsing Codex CLI NDJSON output format.
 * Covers: item.completed, agent_message, turn.started, etc.
 */

const assert = require('assert');
const { parseChunk, parseEvent, parseItem } = require('../src/providers/openai/output-parser');

describe('Codex output parser', () => {
  describe('parseItem', () => {
    it('handles Claude-style assistant messages', () => {
      const item = {
        type: 'message',
        role: 'assistant',
        content: [{ type: 'text', text: 'Hello world' }],
      };
      const result = parseItem(item);
      assert.deepStrictEqual(result, { type: 'text', text: 'Hello world' });
    });

    it('handles Codex-style agent_message', () => {
      const item = {
        type: 'agent_message',
        id: 'item_0',
        text: '{"complexity":"TRIVIAL"}',
      };
      const result = parseItem(item);
      assert.deepStrictEqual(result, { type: 'text', text: '{"complexity":"TRIVIAL"}' });
    });

    it('handles function_call items', () => {
      const item = {
        type: 'function_call',
        name: 'read_file',
        call_id: 'call_123',
        arguments: '{"path":"/tmp/test.txt"}',
      };
      const result = parseItem(item);
      assert.deepStrictEqual(result, {
        type: 'tool_call',
        toolName: 'read_file',
        toolId: 'call_123',
        input: { path: '/tmp/test.txt' },
      });
    });
  });

  describe('parseEvent', () => {
    it('ignores thread.started events', () => {
      const result = parseEvent('{"type":"thread.started","thread_id":"abc"}');
      assert.strictEqual(result, null);
    });

    it('ignores turn.started events', () => {
      const result = parseEvent('{"type":"turn.started"}');
      assert.strictEqual(result, null);
    });

    it('parses item.created events', () => {
      const line = JSON.stringify({
        type: 'item.created',
        item: { type: 'agent_message', text: 'test' },
      });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, { type: 'text', text: 'test' });
    });

    it('parses item.completed events (Codex format)', () => {
      const line = JSON.stringify({
        type: 'item.completed',
        item: { id: 'item_0', type: 'agent_message', text: '{"foo":"bar"}' },
      });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, { type: 'text', text: '{"foo":"bar"}' });
    });

    it('parses turn.completed with usage stats', () => {
      const line = JSON.stringify({
        type: 'turn.completed',
        usage: { input_tokens: 100, output_tokens: 50 },
      });
      const result = parseEvent(line);
      assert.deepStrictEqual(result, {
        type: 'result',
        success: true,
        inputTokens: 100,
        outputTokens: 50,
      });
    });
  });

  describe('parseChunk (full NDJSON stream)', () => {
    it('parses complete Codex output stream', () => {
      const lines = [
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
      ];
      const output = lines.join('\n');

      const events = parseChunk(output);

      assert.strictEqual(events.length, 2);
      assert.deepStrictEqual(events[0], {
        type: 'text',
        text: '{"complexity":"TRIVIAL","taskType":"INQUIRY"}',
      });
      assert.deepStrictEqual(events[1], {
        type: 'result',
        success: true,
        inputTokens: 13202,
        outputTokens: 66,
      });
    });

    it('extracts valid JSON from agent_message text', () => {
      const line = JSON.stringify({
        type: 'item.completed',
        item: {
          type: 'agent_message',
          text: '{"approved":true,"summary":"All good","errors":[]}',
        },
      });

      const events = parseChunk(line);
      assert.strictEqual(events.length, 1);

      const textEvent = events[0];
      assert.strictEqual(textEvent.type, 'text');

      // Verify the JSON is valid and parseable
      const parsed = JSON.parse(textEvent.text);
      assert.deepStrictEqual(parsed, {
        approved: true,
        summary: 'All good',
        errors: [],
      });
    });
  });
});
