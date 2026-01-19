const assert = require('assert');
const { parseChunk } = require('../../src/providers/anthropic/output-parser');

describe('Claude provider parser', () => {
  it('parses text, tool, and result events', () => {
    const chunk = [
      JSON.stringify({
        type: 'stream_event',
        event: {
          type: 'content_block_delta',
          delta: { type: 'text_delta', text: 'Hello' },
        },
      }),
      JSON.stringify({
        type: 'assistant',
        message: {
          content: [
            {
              type: 'tool_use',
              name: 'Read',
              id: 'tool-1',
              input: { path: 'README.md' },
            },
          ],
        },
      }),
      JSON.stringify({
        type: 'user',
        message: {
          content: [
            {
              type: 'tool_result',
              tool_use_id: 'tool-1',
              content: 'OK',
              is_error: false,
            },
          ],
        },
      }),
      JSON.stringify({
        type: 'result',
        subtype: 'success',
        result: 'Done',
        total_cost_usd: 0.01,
        usage: { input_tokens: 5, output_tokens: 7 },
      }),
    ].join('\n');

    const events = parseChunk(chunk);
    const textEvent = events.find((e) => e.type === 'text');
    const toolCall = events.find((e) => e.type === 'tool_call');
    const toolResult = events.find((e) => e.type === 'tool_result');
    const result = events.find((e) => e.type === 'result');

    assert.strictEqual(textEvent.text, 'Hello');
    assert.strictEqual(toolCall.toolName, 'Read');
    assert.strictEqual(toolCall.toolId, 'tool-1');
    assert.strictEqual(toolResult.toolId, 'tool-1');
    assert.strictEqual(result.success, true);
    assert.strictEqual(result.inputTokens, 5);
    assert.strictEqual(result.outputTokens, 7);
  });
});
