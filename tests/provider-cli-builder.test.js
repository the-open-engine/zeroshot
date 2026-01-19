/**
 * Provider CLI Builder Tests
 *
 * Tests the command building logic for each provider, including
 * JSON schema injection when native CLI support is unavailable.
 */

const assert = require('assert');

// ============================================================================
// CODEX PROVIDER
// ============================================================================
describe('Codex CLI Builder', function () {
  const { buildCommand } = require('../src/providers/openai/cli-builder');

  it('should pass --output-schema when CLI supports it', function () {
    const result = buildCommand('test context', {
      jsonSchema: { type: 'object', properties: { foo: { type: 'string' } } },
      cliFeatures: { supportsOutputSchema: true, supportsJson: true },
    });

    assert.ok(result.args.includes('--output-schema'));
    assert.ok(!result.args[result.args.length - 1].includes('OUTPUT FORMAT'));
  });

  it('should inject schema into context when CLI does NOT support --output-schema', function () {
    const schema = { type: 'object', properties: { foo: { type: 'string' } } };
    const result = buildCommand('test context', {
      jsonSchema: schema,
      cliFeatures: { supportsOutputSchema: false, supportsJson: true },
    });

    // Should NOT have --output-schema flag
    assert.ok(!result.args.includes('--output-schema'));

    // Context (last arg) should include schema instructions
    const finalContext = result.args[result.args.length - 1];
    assert.ok(finalContext.includes('## OUTPUT FORMAT (CRITICAL - REQUIRED)'));
    assert.ok(finalContext.includes('You MUST respond with a JSON object'));
    assert.ok(finalContext.includes('"foo"'));
  });

  it('should NOT inject schema when no jsonSchema provided', function () {
    const result = buildCommand('test context', {
      cliFeatures: { supportsOutputSchema: false, supportsJson: true },
    });

    const finalContext = result.args[result.args.length - 1];
    assert.strictEqual(finalContext, 'test context');
    assert.ok(!finalContext.includes('OUTPUT FORMAT'));
  });

  it('should handle string jsonSchema', function () {
    const schemaStr = '{"type":"object","properties":{"bar":{"type":"number"}}}';
    const result = buildCommand('test context', {
      jsonSchema: schemaStr,
      cliFeatures: { supportsOutputSchema: false, supportsJson: true },
    });

    const finalContext = result.args[result.args.length - 1];
    assert.ok(finalContext.includes('## OUTPUT FORMAT'));
    assert.ok(finalContext.includes('"bar"'));
  });

  it('should include --json flag when outputFormat is json', function () {
    const result = buildCommand('test', {
      outputFormat: 'json',
      cliFeatures: { supportsJson: true },
    });

    assert.ok(result.args.includes('--json'));
  });

  it('should include --json flag when outputFormat is stream-json', function () {
    const result = buildCommand('test', {
      outputFormat: 'stream-json',
      cliFeatures: { supportsJson: true },
    });

    assert.ok(result.args.includes('--json'));
  });
});

// ============================================================================
// GEMINI PROVIDER
// ============================================================================
describe('Gemini CLI Builder', function () {
  const { buildCommand } = require('../src/providers/google/cli-builder');

  it('should always inject schema into context (no native support)', function () {
    const schema = { type: 'object', properties: { result: { type: 'string' } } };
    const result = buildCommand('test prompt', {
      jsonSchema: schema,
      cliFeatures: { supportsStreamJson: true },
    });

    // Find the -p argument value (context)
    const pIndex = result.args.indexOf('-p');
    const finalContext = result.args[pIndex + 1];

    assert.ok(finalContext.includes('## OUTPUT FORMAT (CRITICAL - REQUIRED)'));
    assert.ok(finalContext.includes('You MUST respond with a JSON object'));
    assert.ok(finalContext.includes('"result"'));
  });

  it('should NOT inject schema when no jsonSchema provided', function () {
    const result = buildCommand('test prompt', {
      cliFeatures: { supportsStreamJson: true },
    });

    const pIndex = result.args.indexOf('-p');
    const finalContext = result.args[pIndex + 1];

    assert.strictEqual(finalContext, 'test prompt');
    assert.ok(!finalContext.includes('OUTPUT FORMAT'));
  });

  it('should include --output-format stream-json when supported', function () {
    const result = buildCommand('test', {
      outputFormat: 'json',
      cliFeatures: { supportsStreamJson: true },
    });

    assert.ok(result.args.includes('--output-format'));
    assert.ok(result.args.includes('stream-json'));
  });
});

// ============================================================================
// OPENCODE PROVIDER
// ============================================================================
describe('Opencode CLI Builder', function () {
  const { buildCommand } = require('../src/providers/opencode/cli-builder');

  it('should inject schema into context when jsonSchema provided', function () {
    const schema = { type: 'object', properties: { result: { type: 'string' } } };
    const result = buildCommand('test prompt', {
      jsonSchema: schema,
      cliFeatures: { supportsJson: true },
    });

    const finalContext = result.args[result.args.length - 1];
    assert.ok(finalContext.includes('## OUTPUT FORMAT (CRITICAL - REQUIRED)'));
    assert.ok(finalContext.includes('You MUST respond with a JSON object'));
    assert.ok(finalContext.includes('"result"'));
  });

  it('should include --format json when outputFormat is json or stream-json', function () {
    const result = buildCommand('test', {
      outputFormat: 'json',
      cliFeatures: { supportsJson: true },
    });

    assert.ok(result.args.includes('--format'));
    assert.ok(result.args.includes('json'));
  });

  it('should include model and variant when provided', function () {
    const result = buildCommand('test', {
      modelSpec: { model: 'opencode/glm-4.7-free', reasoningEffort: 'high' },
      cliFeatures: { supportsJson: true, supportsVariant: true },
    });

    assert.ok(result.args.includes('--model'));
    assert.ok(result.args.includes('opencode/glm-4.7-free'));
    assert.ok(result.args.includes('--variant'));
    assert.ok(result.args.includes('high'));
  });
});

// ============================================================================
// CLAUDE PROVIDER
// ============================================================================
describe('Claude CLI Builder', function () {
  const { buildCommand } = require('../src/providers/anthropic/cli-builder');

  it('should pass --json-schema when CLI supports it', function () {
    const schema = { type: 'object', properties: { foo: { type: 'string' } } };
    const result = buildCommand('test context', {
      jsonSchema: schema,
      outputFormat: 'json',
      cliFeatures: { supportsJsonSchema: true },
    });

    assert.ok(result.args.includes('--json-schema'));
  });

  it('should NOT pass --json-schema when outputFormat is not json', function () {
    const schema = { type: 'object', properties: { foo: { type: 'string' } } };
    const result = buildCommand('test context', {
      jsonSchema: schema,
      outputFormat: 'stream-json',
      cliFeatures: { supportsJsonSchema: true },
    });

    assert.ok(!result.args.includes('--json-schema'));
  });

  it('should NOT pass --json-schema when CLI does not support it', function () {
    const schema = { type: 'object', properties: { foo: { type: 'string' } } };
    const result = buildCommand('test context', {
      jsonSchema: schema,
      outputFormat: 'json',
      cliFeatures: { supportsJsonSchema: false },
    });

    assert.ok(!result.args.includes('--json-schema'));
  });
});

// ============================================================================
// REGRESSION TESTS
// ============================================================================
describe('Regression Tests', function () {
  it('REGRESSION: Codex planner returns markdown when schema not enforced', function () {
    // This was the bug: Codex CLI doesn't support --output-schema,
    // so schema was silently dropped and model returned markdown
    const { buildCommand } = require('../src/providers/openai/cli-builder');

    const plannerSchema = {
      type: 'object',
      properties: {
        plan: { type: 'string' },
        summary: { type: 'string' },
        filesAffected: { type: 'array', items: { type: 'string' } },
      },
      required: ['plan', 'summary'],
    };

    const result = buildCommand('Create a plan for implementing feature X', {
      jsonSchema: plannerSchema,
      outputFormat: 'json',
      cliFeatures: { supportsOutputSchema: false, supportsJson: true },
    });

    const finalContext = result.args[result.args.length - 1];

    // Must have schema injection
    assert.ok(
      finalContext.includes('OUTPUT FORMAT'),
      'Schema instructions must be injected when CLI lacks native support'
    );

    // Must tell model to output JSON only
    assert.ok(
      finalContext.includes('ONLY valid JSON'),
      'Must explicitly tell model to output only JSON'
    );

    // Must include the actual schema
    assert.ok(finalContext.includes('"plan"'), 'Schema must include plan property');
    assert.ok(finalContext.includes('"summary"'), 'Schema must include summary property');
  });

  it('REGRESSION: Gemini conductor returns text when schema not enforced', function () {
    // Gemini CLI has no --output-schema support at all
    const { buildCommand } = require('../src/providers/google/cli-builder');

    const conductorSchema = {
      type: 'object',
      properties: {
        complexity: { type: 'string', enum: ['TRIVIAL', 'SIMPLE', 'STANDARD', 'CRITICAL'] },
        taskType: { type: 'string', enum: ['INQUIRY', 'TASK', 'DEBUG'] },
        reasoning: { type: 'string' },
      },
      required: ['complexity', 'taskType', 'reasoning'],
    };

    const result = buildCommand('Classify this task', {
      jsonSchema: conductorSchema,
      outputFormat: 'json',
      cliFeatures: { supportsStreamJson: true },
    });

    const pIndex = result.args.indexOf('-p');
    const finalContext = result.args[pIndex + 1];

    // Must have schema injection
    assert.ok(
      finalContext.includes('OUTPUT FORMAT'),
      'Schema instructions must always be injected for Gemini provider'
    );

    // Must include the actual schema properties
    assert.ok(finalContext.includes('"complexity"'), 'Schema must include complexity');
    assert.ok(finalContext.includes('"taskType"'), 'Schema must include taskType');
  });
});
