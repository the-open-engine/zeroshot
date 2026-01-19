/**
 * Output Reformatter Tests
 *
 * Tests the reformatting utilities for converting non-JSON output to JSON.
 *
 * STATUS: SDK NOT IMPLEMENTED - reformatOutput() always throws.
 * The helper functions (buildReformatPrompt, validateAgainstSchema) still work.
 */

const assert = require('assert');
const {
  reformatOutput,
  buildReformatPrompt,
  validateAgainstSchema,
  DEFAULT_MAX_ATTEMPTS,
} = require('../src/agent/output-reformatter');

describe('Output Reformatter', function () {
  describe('buildReformatPrompt', function () {
    it('should build prompt with schema and output', function () {
      const schema = { type: 'object', properties: { foo: { type: 'string' } } };
      const rawOutput = 'Here is the result: foo is bar';

      const prompt = buildReformatPrompt(rawOutput, schema);

      assert.ok(prompt.includes('Convert this text into a JSON object'));
      assert.ok(prompt.includes('"foo"'));
      assert.ok(prompt.includes('Here is the result'));
      assert.ok(prompt.includes('Start with { end with }'));
    });

    it('should include previous error when provided', function () {
      const schema = { type: 'object', properties: { x: { type: 'number' } } };
      const rawOutput = 'The value is 42';
      const previousError = 'Missing required field: x';

      const prompt = buildReformatPrompt(rawOutput, schema, previousError);

      assert.ok(prompt.includes('PREVIOUS ATTEMPT FAILED'));
      assert.ok(prompt.includes('Missing required field: x'));
      assert.ok(prompt.includes('Fix this issue'));
    });

    it('should truncate very long outputs', function () {
      const schema = { type: 'object' };
      const rawOutput = 'x'.repeat(10000);

      const prompt = buildReformatPrompt(rawOutput, schema);

      // Should truncate to last 4000 chars
      assert.ok(prompt.length < 10000);
      assert.ok(prompt.includes('xxxx')); // Contains truncated content
    });
  });

  describe('validateAgainstSchema', function () {
    it('should return null for valid object', function () {
      const schema = {
        type: 'object',
        properties: {
          name: { type: 'string' },
          age: { type: 'number' },
        },
        required: ['name'],
      };
      const parsed = { name: 'Alice', age: 30 };

      const error = validateAgainstSchema(parsed, schema);

      assert.strictEqual(error, null);
    });

    it('should return error for missing required field', function () {
      const schema = {
        type: 'object',
        properties: {
          name: { type: 'string' },
        },
        required: ['name'],
      };
      const parsed = { other: 'value' };

      const error = validateAgainstSchema(parsed, schema);

      assert.ok(error !== null);
      assert.ok(error.includes('name'));
    });

    it('should return error for wrong type', function () {
      const schema = {
        type: 'object',
        properties: {
          count: { type: 'number' },
        },
      };
      const parsed = { count: 'not a number' };

      const error = validateAgainstSchema(parsed, schema);

      assert.ok(error !== null);
      assert.ok(error.includes('number'));
    });

    it('should return error for invalid enum value', function () {
      const schema = {
        type: 'object',
        properties: {
          status: { type: 'string', enum: ['ACTIVE', 'INACTIVE'] },
        },
      };
      const parsed = { status: 'UNKNOWN' };

      const error = validateAgainstSchema(parsed, schema);

      assert.ok(error !== null);
    });
  });

  describe('DEFAULT_MAX_ATTEMPTS', function () {
    it('should be 3', function () {
      assert.strictEqual(DEFAULT_MAX_ATTEMPTS, 3);
    });
  });

  describe('reformatOutput', function () {
    // SDK not implemented - reformatOutput() always throws immediately

    it('should throw SDK not implemented error', async function () {
      await assert.rejects(
        () =>
          reformatOutput({
            rawOutput: 'Some text',
            schema: { type: 'object', properties: { x: { type: 'number' } } },
            providerName: 'claude',
          }),
        /SDK not implemented/
      );
    });

    it('should include provider name in error', async function () {
      await assert.rejects(
        () =>
          reformatOutput({
            rawOutput: 'Some text',
            schema: { type: 'object' },
            providerName: 'codex',
          }),
        /provider "codex"/
      );
    });

    it('should include raw output snippet in error', async function () {
      await assert.rejects(
        () =>
          reformatOutput({
            rawOutput: 'This is the raw output text',
            schema: { type: 'object' },
            providerName: 'gemini',
          }),
        /This is the raw output text/
      );
    });
  });
});
