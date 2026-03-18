const assert = require('assert');
const { parseChunk } = require('../../src/providers/novita/output-parser');

describe('Novita provider parser', () => {
  it('parses assistant text output', () => {
    const fixture = require('fs').readFileSync(
      require('path').join(__dirname, '..', 'fixtures', 'novita', 'text.jsonl'),
      'utf8'
    );
    const events = parseChunk(fixture);

    const textEvent = events.find((e) => e.type === 'text');
    const result = events.find((e) => e.type === 'result');

    assert.strictEqual(textEvent.text, 'Hello from Novita.');
    assert.strictEqual(result.success, true);
    assert.strictEqual(result.inputTokens, 12);
    assert.strictEqual(result.outputTokens, 4);
  });

  it('parses tool call and result output', () => {
    const fixture = require('fs').readFileSync(
      require('path').join(__dirname, '..', 'fixtures', 'novita', 'tool.jsonl'),
      'utf8'
    );
    const events = parseChunk(fixture);

    const toolCall = events.find((e) => e.type === 'tool_call');
    const toolResult = events.find((e) => e.type === 'tool_result');
    const result = events.find((e) => e.type === 'result');

    assert.strictEqual(toolCall.toolName, 'read_file');
    assert.strictEqual(toolCall.toolId, 'call-1');
    assert.strictEqual(toolCall.input.path, 'README.md');
    assert.strictEqual(toolResult.toolId, 'call-1');
    assert.strictEqual(toolResult.content, 'README contents');
    assert.strictEqual(result.outputTokens, 9);
  });
});
