/**
 * Output Extraction Unit Tests
 *
 * Tests the clean extraction pipeline for multi-provider JSON extraction.
 * Each strategy is tested in isolation, then integration tests verify
 * the full pipeline works correctly.
 */

const assert = require('assert');
const {
  extractJsonFromOutput,
  extractFromResultWrapper,
  extractFromTextEvents,
  extractFromMarkdown,
  extractDirectJson,
  stripTimestamp,
} = require('../src/agent/output-extraction');

describe('Output Extraction Module', function () {
  // ============================================================================
  // UTILITY FUNCTIONS
  // ============================================================================
  describe('stripTimestamp', function () {
    it('should strip epoch timestamp prefix', function () {
      const result = stripTimestamp('[1768207508291]{"type":"result"}');
      assert.strictEqual(result, '{"type":"result"}');
    });

    it('should handle lines without timestamp', function () {
      const result = stripTimestamp('{"type":"result"}');
      assert.strictEqual(result, '{"type":"result"}');
    });

    it('should handle empty input', function () {
      assert.strictEqual(stripTimestamp(''), '');
      assert.strictEqual(stripTimestamp(null), '');
      assert.strictEqual(stripTimestamp(undefined), '');
    });

    it('should strip CRLF line endings', function () {
      const result = stripTimestamp('[1768207508291]content\r');
      assert.strictEqual(result, 'content');
    });

    it('should trim whitespace', function () {
      const result = stripTimestamp('  [1768207508291]content  ');
      assert.strictEqual(result, 'content');
    });
  });

  // ============================================================================
  // STRATEGY 1: RESULT WRAPPER EXTRACTION
  // ============================================================================
  describe('extractFromResultWrapper', function () {
    it('should extract from result field (object)', function () {
      const output = '{"type":"result","result":{"foo":"bar"}}';
      const result = extractFromResultWrapper(output);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should extract from structured_output field', function () {
      const output = '{"type":"result","structured_output":{"foo":"bar"}}';
      const result = extractFromResultWrapper(output);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should prefer structured_output over result', function () {
      const output = '{"type":"result","structured_output":{"a":1},"result":{"b":2}}';
      const result = extractFromResultWrapper(output);
      assert.deepStrictEqual(result, { a: 1 });
    });

    it('should handle result as string containing JSON', function () {
      const output = '{"type":"result","result":"{\\"foo\\":\\"bar\\"}"}';
      const result = extractFromResultWrapper(output);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should handle result as string containing markdown JSON', function () {
      const output = '{"type":"result","result":"```json\\n{\\"foo\\":\\"bar\\"}\\n```"}';
      const result = extractFromResultWrapper(output);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should handle timestamp prefixed lines', function () {
      const output = '[1768207508291]{"type":"result","result":{"foo":"bar"}}';
      const result = extractFromResultWrapper(output);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should find result line in NDJSON', function () {
      const output = `{"type":"system"}
{"type":"assistant"}
{"type":"result","result":{"foo":"bar"}}`;
      const result = extractFromResultWrapper(output);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should return null for type:result WITHOUT content', function () {
      // This is the critical case for non-Claude providers
      const output = '{"type":"result","success":true}';
      const result = extractFromResultWrapper(output);
      assert.strictEqual(result, null);
    });

    it('should return null for non-result types', function () {
      const output = '{"type":"assistant","content":"hello"}';
      const result = extractFromResultWrapper(output);
      assert.strictEqual(result, null);
    });

    it('should return null for empty input', function () {
      assert.strictEqual(extractFromResultWrapper(''), null);
      assert.strictEqual(extractFromResultWrapper('not json'), null);
    });
  });

  // ============================================================================
  // STRATEGY 2: TEXT EVENTS EXTRACTION
  // ============================================================================
  describe('extractFromTextEvents', function () {
    it('should extract from Claude text events', function () {
      // Claude stream_event format
      const output = `{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"{\\"foo\\":\\"bar\\"}"}}}`;
      const result = extractFromTextEvents(output, 'claude');
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should extract from Codex item.created events', function () {
      const output = `{"type":"item.created","item":{"type":"message","role":"assistant","content":[{"type":"text","text":"{\\"foo\\":\\"bar\\"}"}]}}`;
      const result = extractFromTextEvents(output, 'codex');
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should extract from Gemini message events', function () {
      const output = `{"type":"message","role":"assistant","content":"{\\"foo\\":\\"bar\\"}"}`;
      const result = extractFromTextEvents(output, 'gemini');
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should concatenate multiple text events', function () {
      // Multiple text deltas that form a complete JSON
      const output = `{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"{\\"foo\\":"}}}
{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"\\"bar\\"}"}}}`;
      const result = extractFromTextEvents(output, 'claude');
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should return null when no text events', function () {
      const output = '{"type":"system","subtype":"init"}';
      const result = extractFromTextEvents(output, 'claude');
      assert.strictEqual(result, null);
    });

    it('should handle text containing markdown', function () {
      const output = `{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"\`\`\`json\\n{\\"foo\\":\\"bar\\"}\\n\`\`\`"}}}`;
      const result = extractFromTextEvents(output, 'claude');
      assert.deepStrictEqual(result, { foo: 'bar' });
    });
  });

  // ============================================================================
  // STRATEGY 3: MARKDOWN EXTRACTION
  // ============================================================================
  describe('extractFromMarkdown', function () {
    it('should extract JSON from markdown code block', function () {
      const text = '```json\n{"foo":"bar"}\n```';
      const result = extractFromMarkdown(text);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should handle surrounding text', function () {
      const text = 'Here is the result:\n\n```json\n{"foo":"bar"}\n```\n\nDone.';
      const result = extractFromMarkdown(text);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should handle multi-line JSON in markdown', function () {
      const text = '```json\n{\n  "foo": "bar",\n  "baz": 123\n}\n```';
      const result = extractFromMarkdown(text);
      assert.deepStrictEqual(result, { foo: 'bar', baz: 123 });
    });

    it('should handle no whitespace after json tag', function () {
      const text = '```json{"foo":"bar"}```';
      const result = extractFromMarkdown(text);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should return null for non-markdown text', function () {
      const text = '{"foo":"bar"}';
      const result = extractFromMarkdown(text);
      assert.strictEqual(result, null);
    });

    it('should return null for invalid JSON in markdown', function () {
      const text = '```json\n{invalid json}\n```';
      const result = extractFromMarkdown(text);
      assert.strictEqual(result, null);
    });

    it('should return null for empty input', function () {
      assert.strictEqual(extractFromMarkdown(''), null);
      assert.strictEqual(extractFromMarkdown(null), null);
    });
  });

  // ============================================================================
  // STRATEGY 4: DIRECT JSON EXTRACTION
  // ============================================================================
  describe('extractDirectJson', function () {
    it('should parse single-line JSON', function () {
      const text = '{"foo":"bar"}';
      const result = extractDirectJson(text);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should parse multi-line JSON', function () {
      const text = `{
  "foo": "bar",
  "baz": 123
}`;
      const result = extractDirectJson(text);
      assert.deepStrictEqual(result, { foo: 'bar', baz: 123 });
    });

    it('should handle whitespace around JSON', function () {
      const text = '  \n  {"foo":"bar"}  \n  ';
      const result = extractDirectJson(text);
      assert.deepStrictEqual(result, { foo: 'bar' });
    });

    it('should return null for arrays', function () {
      const text = '[1, 2, 3]';
      const result = extractDirectJson(text);
      assert.strictEqual(result, null);
    });

    it('should return null for primitives', function () {
      assert.strictEqual(extractDirectJson('"string"'), null);
      assert.strictEqual(extractDirectJson('123'), null);
      assert.strictEqual(extractDirectJson('true'), null);
      assert.strictEqual(extractDirectJson('null'), null);
    });

    it('should return null for invalid JSON', function () {
      assert.strictEqual(extractDirectJson('{invalid}'), null);
      assert.strictEqual(extractDirectJson('not json'), null);
    });

    it('should return null for empty input', function () {
      assert.strictEqual(extractDirectJson(''), null);
      assert.strictEqual(extractDirectJson('   '), null);
      assert.strictEqual(extractDirectJson(null), null);
    });
  });

  // ============================================================================
  // FULL PIPELINE INTEGRATION
  // ============================================================================
  describe('extractJsonFromOutput (full pipeline)', function () {
    describe('Claude Provider', function () {
      it('should extract from result wrapper', function () {
        const output = '{"type":"result","result":{"complexity":"SIMPLE","taskType":"INQUIRY"}}';
        const result = extractJsonFromOutput(output, 'claude');
        assert.strictEqual(result.complexity, 'SIMPLE');
        assert.strictEqual(result.taskType, 'INQUIRY');
      });

      it('should extract from NDJSON with result line', function () {
        const output = `[1768207505878]{"type":"system","subtype":"init"}
[1768207506000]{"type":"assistant","message":{"content":[]}}
[1768207508291]{"type":"result","result":{"complexity":"STANDARD","taskType":"TASK"}}`;
        const result = extractJsonFromOutput(output, 'claude');
        assert.strictEqual(result.complexity, 'STANDARD');
      });

      it('should extract from text events when no result wrapper', function () {
        const output =
          '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"{\\"complexity\\":\\"TRIVIAL\\"}"}}}';
        const result = extractJsonFromOutput(output, 'claude');
        assert.strictEqual(result.complexity, 'TRIVIAL');
      });
    });

    describe('Codex Provider', function () {
      it('should extract from text events (not turn.completed)', function () {
        const output = `{"type":"item.created","item":{"type":"message","role":"assistant","content":[{"type":"text","text":"{\\"complexity\\":\\"SIMPLE\\",\\"taskType\\":\\"INQUIRY\\"}"}]}}
{"type":"turn.completed","usage":{"input_tokens":100}}`;
        const result = extractJsonFromOutput(output, 'codex');
        assert.strictEqual(result.complexity, 'SIMPLE');
        assert.strictEqual(result.taskType, 'INQUIRY');
      });

      it('should extract raw JSON output', function () {
        const output = '{"complexity":"STANDARD","taskType":"TASK"}';
        const result = extractJsonFromOutput(output, 'codex');
        assert.strictEqual(result.complexity, 'STANDARD');
      });

      it('should extract multi-line JSON', function () {
        const output = `{
  "complexity": "CRITICAL",
  "taskType": "DEBUG"
}`;
        const result = extractJsonFromOutput(output, 'codex');
        assert.strictEqual(result.complexity, 'CRITICAL');
        assert.strictEqual(result.taskType, 'DEBUG');
      });
    });

    describe('Gemini Provider', function () {
      it('should extract from message events', function () {
        const output = `{"type":"message","role":"assistant","content":"{\\"complexity\\":\\"SIMPLE\\"}"}
{"type":"result","success":true}`;
        const result = extractJsonFromOutput(output, 'gemini');
        assert.strictEqual(result.complexity, 'SIMPLE');
      });

      it('should extract raw JSON output', function () {
        const output = '{"complexity":"STANDARD","taskType":"TASK"}';
        const result = extractJsonFromOutput(output, 'gemini');
        assert.strictEqual(result.complexity, 'STANDARD');
      });
    });

    describe('Edge Cases', function () {
      it('should return null for empty output', function () {
        assert.strictEqual(extractJsonFromOutput('', 'claude'), null);
        assert.strictEqual(extractJsonFromOutput(null, 'claude'), null);
      });

      it('should return null for Task not found', function () {
        assert.strictEqual(extractJsonFromOutput('Task not found', 'claude'), null);
      });

      it('should return null for Process terminated', function () {
        assert.strictEqual(extractJsonFromOutput('Process terminated by signal', 'claude'), null);
      });

      it('should return null for plain text', function () {
        assert.strictEqual(extractJsonFromOutput('Just some text', 'claude'), null);
      });

      it('should handle markdown in any provider', function () {
        const output = 'Here is the result:\n\n```json\n{"foo":"bar"}\n```\n\nDone.';
        assert.deepStrictEqual(extractJsonFromOutput(output, 'claude'), { foo: 'bar' });
        assert.deepStrictEqual(extractJsonFromOutput(output, 'codex'), { foo: 'bar' });
        assert.deepStrictEqual(extractJsonFromOutput(output, 'gemini'), { foo: 'bar' });
      });
    });

    describe('Strategy Priority', function () {
      it('should prefer result wrapper over text events', function () {
        // Both result wrapper and text events present - wrapper should win
        const output = `{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"{\\"source\\":\\"text\\"}"}}}
{"type":"result","result":{"source":"wrapper"}}`;
        const result = extractJsonFromOutput(output, 'claude');
        assert.strictEqual(result.source, 'wrapper');
      });

      it('should prefer text events over markdown', function () {
        // Text events should be extracted before trying markdown
        const output =
          '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"{\\"source\\":\\"text\\"}"}}}';
        const result = extractJsonFromOutput(output, 'claude');
        assert.strictEqual(result.source, 'text');
      });
    });
  });

  // ============================================================================
  // REGRESSION TESTS
  // ============================================================================
  describe('Regression Tests', function () {
    it('REGRESSION: Codex turn.completed without result field', function () {
      // The original bug: turn.completed has type:result but no result field
      const output = `{"type":"thread.started"}
{"type":"item.created","item":{"type":"message","role":"assistant","content":[{"type":"text","text":"{\\"complexity\\":\\"SIMPLE\\",\\"taskType\\":\\"INQUIRY\\",\\"reasoning\\":\\"Quick question\\"}"}]}}
{"type":"turn.completed","usage":{"input_tokens":3194,"output_tokens":50}}`;
      const result = extractJsonFromOutput(output, 'codex');
      assert.strictEqual(result.complexity, 'SIMPLE');
      assert.strictEqual(result.taskType, 'INQUIRY');
      assert.ok(result.reasoning);
    });

    it('REGRESSION: Gemini result event without result field', function () {
      const output = `{"type":"message","role":"assistant","content":"{\\"complexity\\":\\"CRITICAL\\",\\"taskType\\":\\"DEBUG\\"}"}
{"type":"result","success":true}`;
      const result = extractJsonFromOutput(output, 'gemini');
      assert.strictEqual(result.complexity, 'CRITICAL');
      assert.strictEqual(result.taskType, 'DEBUG');
    });

    it('REGRESSION: Multi-line pretty-printed JSON', function () {
      const output = `{
  "complexity": "STANDARD",
  "taskType": "TASK",
  "reasoning": "Multi-file implementation"
}`;
      const result = extractJsonFromOutput(output, 'codex');
      assert.strictEqual(result.complexity, 'STANDARD');
      assert.strictEqual(result.taskType, 'TASK');
    });

    it('REGRESSION: Markdown code block extraction', function () {
      const output = `Here is my analysis:

\`\`\`json
{"complexity":"SIMPLE","taskType":"TASK","reasoning":"Add new feature"}
\`\`\`

Done.`;
      const result = extractJsonFromOutput(output, 'claude');
      assert.strictEqual(result.complexity, 'SIMPLE');
      assert.strictEqual(result.taskType, 'TASK');
    });

    it('REGRESSION: Result string containing markdown', function () {
      const output =
        '{"type":"result","result":"```json\\n{\\"complexity\\":\\"TRIVIAL\\"}\\n```"}';
      const result = extractJsonFromOutput(output, 'claude');
      assert.strictEqual(result.complexity, 'TRIVIAL');
    });

    it('REGRESSION #52: Claude assistant message with text blocks (Haiku)', function () {
      // Issue #52: Haiku returns JSON in type:assistant message with type:text content blocks
      // The output parser was ignoring these blocks, only handling tool_use and thinking
      const output = `{"type":"system","subtype":"init"}
{"type":"assistant","message":{"content":[{"type":"text","text":"{\\"complexity\\":\\"SIMPLE\\",\\"taskType\\":\\"INQUIRY\\",\\"reasoning\\":\\"Quick question\\"}"}]}}
{"type":"result","success":true}`;
      const result = extractJsonFromOutput(output, 'claude');
      assert.strictEqual(result.complexity, 'SIMPLE');
      assert.strictEqual(result.taskType, 'INQUIRY');
      assert.ok(result.reasoning);
    });

    it('REGRESSION #52: Claude assistant message with multiple text blocks', function () {
      // Haiku may split output across multiple text blocks
      const output = `{"type":"assistant","message":{"content":[{"type":"text","text":"{\\"complexity\\":"},{"type":"text","text":"\\"STANDARD\\",\\"taskType\\":\\"DEBUG\\",\\"reasoning\\":\\"Fix bug\\"}"}]}}`;
      const result = extractJsonFromOutput(output, 'claude');
      assert.strictEqual(result.complexity, 'STANDARD');
      assert.strictEqual(result.taskType, 'DEBUG');
    });

    it('REGRESSION #52: Claude assistant message text blocks alongside tool_use', function () {
      // Real-world scenario: text blocks mixed with tool_use
      const output = `{"type":"assistant","message":{"content":[{"type":"text","text":"{\\"complexity\\":\\"TRIVIAL\\",\\"taskType\\":\\"TASK\\",\\"reasoning\\":\\"Simple change\\"}"},{"type":"tool_use","id":"tool1","name":"Read","input":{}}]}}`;
      const result = extractJsonFromOutput(output, 'claude');
      assert.strictEqual(result.complexity, 'TRIVIAL');
      assert.strictEqual(result.taskType, 'TASK');
    });
  });
});
