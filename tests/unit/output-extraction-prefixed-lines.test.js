const assert = require('assert');
const { extractJsonFromOutput } = require('../../src/agent/output-extraction');

describe('Output Extraction - Prefixed Log Lines', () => {
  it('extracts JSON from Gemini stream-json when lines have agent prefixes', () => {
    const output = [
      'validator       | {"type":"init","timestamp":"2026-01-12T00:00:00.000Z","session_id":"x","model":"auto"}',
      'validator       | {"type":"message","timestamp":"2026-01-12T00:00:01.000Z","role":"assistant","content":"{\\"approved\\":true,\\"summary\\":\\"ok\\",\\"errors\\":[]}","delta":true}',
      'validator       | {"type":"result","timestamp":"2026-01-12T00:00:02.000Z","status":"success","stats":{"total_tokens":1}}',
    ].join('\n');

    const parsed = extractJsonFromOutput(output, 'google');

    assert.deepStrictEqual(parsed, { approved: true, summary: 'ok', errors: [] });
  });

  it('extracts JSON from Gemini stream-json when lines have timestamp + agent prefixes', () => {
    const output = [
      '[1700000000000]validator | {"type":"message","role":"assistant","content":"{\\"approved\\":true,\\"summary\\":\\"ok\\",\\"errors\\":[]}","delta":true}',
      '[1700000000001]validator | {"type":"result","status":"success","stats":{"total_tokens":1}}',
    ].join('\n');

    const parsed = extractJsonFromOutput(output, 'google');

    assert.deepStrictEqual(parsed, { approved: true, summary: 'ok', errors: [] });
  });

  it('extracts JSON from Opencode when lines have agent prefixes', () => {
    const output = [
      'investigator | {"type":"text","part":{"type":"text","text":"{\\"foo\\":\\"bar\\"}"}}',
      'investigator | {"type":"step_finish","part":{"type":"step-finish","tokens":{"input":1,"output":1}}}',
    ].join('\n');

    const parsed = extractJsonFromOutput(output, 'opencode');

    assert.deepStrictEqual(parsed, { foo: 'bar' });
  });

  it('extracts JSON from Opencode when lines have timestamp + agent prefixes', () => {
    const output = [
      '[1700000000000]investigator | {"type":"text","part":{"type":"text","text":"Working..."}}',
      '[1700000000001]investigator | {"type":"text","part":{"type":"text","text":"{\\"foo\\":\\"bar\\"}"}}',
    ].join('\n');

    const parsed = extractJsonFromOutput(output, 'opencode');

    assert.deepStrictEqual(parsed, { foo: 'bar' });
  });
});
