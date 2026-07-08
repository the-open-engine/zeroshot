/**
 * Provider helper command builder tests.
 */

const assert = require('assert');
const fs = require('fs');
const helper = require('../lib/agent-cli-provider');

const createdTempFiles = new Set();

afterEach(() => {
  for (const file of createdTempFiles) {
    try {
      if (fs.existsSync(file)) fs.unlinkSync(file);
    } catch {
      // best-effort test cleanup
    }
  }
  createdTempFiles.clear();
});

function buildCommand(provider, context, options = {}) {
  const spec = helper.buildProviderCommand(provider, context, options);
  for (const file of spec.cleanup || []) createdTempFiles.add(file);
  return spec;
}

describe('Codex provider helper builder', function () {
  it('warns when unsupported session control options are ignored', function () {
    const resumed = buildCommand('codex', 'test context', {
      resumeSessionId: 'session-123',
    });
    assert.ok(!resumed.args.includes('--resume'));
    assert.ok(
      resumed.warnings.some(
        (warning) =>
          warning.code === 'unsupported-session-control' &&
          warning.message === 'resume/continue is only supported for Claude CLI; ignoring.'
      )
    );

    const continued = buildCommand('codex', 'test context', {
      continueSession: true,
    });
    assert.ok(!continued.args.includes('--continue'));
    assert.ok(continued.warnings.some((warning) => warning.code === 'unsupported-session-control'));
  });

  it('passes --output-schema when CLI supports it', function () {
    const result = buildCommand('codex', 'test context', {
      jsonSchema: { type: 'object', properties: { foo: { type: 'string' } } },
      cliFeatures: { supportsOutputSchema: true, supportsJson: true },
    });

    assert.ok(result.args.includes('--output-schema'));
    assert.ok(!result.args[result.args.length - 1].includes('OUTPUT FORMAT'));
    assert.strictEqual(result.cleanupMetadata.length, 1);
  });

  it('injects schema into context when CLI does not support --output-schema', function () {
    const schema = { type: 'object', properties: { foo: { type: 'string' } } };
    const result = buildCommand('codex', 'test context', {
      jsonSchema: schema,
      cliFeatures: { supportsOutputSchema: false, supportsJson: true },
    });

    assert.ok(!result.args.includes('--output-schema'));
    const finalContext = result.args[result.args.length - 1];
    assert.ok(finalContext.includes('## OUTPUT FORMAT (CRITICAL - REQUIRED)'));
    assert.ok(finalContext.includes('You MUST respond with a JSON object'));
    assert.ok(finalContext.includes('"foo"'));
  });

  it('does not inject schema when no jsonSchema is provided', function () {
    const result = buildCommand('codex', 'test context', {
      cliFeatures: { supportsOutputSchema: false, supportsJson: true },
    });

    const finalContext = result.args[result.args.length - 1];
    assert.strictEqual(finalContext, 'test context');
    assert.ok(!finalContext.includes('OUTPUT FORMAT'));
  });

  it('handles string jsonSchema', function () {
    const schemaStr = '{"type":"object","properties":{"bar":{"type":"number"}}}';
    const result = buildCommand('codex', 'test context', {
      jsonSchema: schemaStr,
      cliFeatures: { supportsOutputSchema: false, supportsJson: true },
    });

    const finalContext = result.args[result.args.length - 1];
    assert.ok(finalContext.includes('## OUTPUT FORMAT'));
    assert.ok(finalContext.includes('"bar"'));
  });

  it('includes --json flag when outputFormat is json', function () {
    const result = buildCommand('codex', 'test', {
      outputFormat: 'json',
      cliFeatures: { supportsJson: true },
    });

    assert.ok(result.args.includes('--json'));
  });

  it('includes --json flag when outputFormat is stream-json', function () {
    const result = buildCommand('codex', 'test', {
      outputFormat: 'stream-json',
      cliFeatures: { supportsJson: true },
    });

    assert.ok(result.args.includes('--json'));
  });
});

describe('Gemini provider helper builder', function () {
  it('warns when unsupported session control options are ignored', function () {
    const resumed = buildCommand('gemini', 'test context', {
      resumeSessionId: 'session-123',
    });
    assert.ok(!resumed.args.includes('--resume'));
    assert.ok(
      resumed.warnings.some(
        (warning) =>
          warning.code === 'unsupported-session-control' &&
          warning.message === 'resume/continue is only supported for Claude CLI; ignoring.'
      )
    );

    const continued = buildCommand('gemini', 'test context', {
      continueSession: true,
    });
    assert.ok(!continued.args.includes('--continue'));
    assert.ok(continued.warnings.some((warning) => warning.code === 'unsupported-session-control'));
  });

  it('always injects schema into context', function () {
    const schema = { type: 'object', properties: { result: { type: 'string' } } };
    const result = buildCommand('gemini', 'test prompt', {
      jsonSchema: schema,
      cliFeatures: { supportsStreamJson: true },
    });

    const pIndex = result.args.indexOf('-p');
    const finalContext = result.args[pIndex + 1];

    assert.ok(finalContext.includes('## OUTPUT FORMAT (CRITICAL - REQUIRED)'));
    assert.ok(finalContext.includes('You MUST respond with a JSON object'));
    assert.ok(finalContext.includes('"result"'));
  });

  it('does not inject schema when no jsonSchema is provided', function () {
    const result = buildCommand('gemini', 'test prompt', {
      cliFeatures: { supportsStreamJson: true },
    });

    const pIndex = result.args.indexOf('-p');
    const finalContext = result.args[pIndex + 1];

    assert.strictEqual(finalContext, 'test prompt');
    assert.ok(!finalContext.includes('OUTPUT FORMAT'));
  });

  it('includes --output-format stream-json when supported', function () {
    const result = buildCommand('gemini', 'test', {
      outputFormat: 'json',
      cliFeatures: { supportsStreamJson: true },
    });

    assert.ok(result.args.includes('--output-format'));
    assert.ok(result.args.includes('stream-json'));
  });
});

describe('Opencode provider helper builder', function () {
  it('warns when unsupported session control options are ignored', function () {
    const resumed = buildCommand('opencode', 'test context', {
      resumeSessionId: 'session-123',
    });
    assert.ok(!resumed.args.includes('--resume'));
    assert.ok(
      resumed.warnings.some(
        (warning) =>
          warning.code === 'unsupported-session-control' &&
          warning.message === 'resume/continue is only supported for Claude CLI; ignoring.'
      )
    );

    const continued = buildCommand('opencode', 'test context', {
      continueSession: true,
    });
    assert.ok(!continued.args.includes('--continue'));
    assert.ok(continued.warnings.some((warning) => warning.code === 'unsupported-session-control'));
  });

  it('injects schema into context when jsonSchema is provided', function () {
    const schema = { type: 'object', properties: { result: { type: 'string' } } };
    const result = buildCommand('opencode', 'test prompt', {
      jsonSchema: schema,
      cliFeatures: { supportsJson: true },
    });

    const finalContext = result.args[result.args.length - 1];
    assert.ok(finalContext.includes('## OUTPUT FORMAT (CRITICAL - REQUIRED)'));
    assert.ok(finalContext.includes('You MUST respond with a JSON object'));
    assert.ok(finalContext.includes('"result"'));
  });

  it('includes --format json when outputFormat is json or stream-json', function () {
    const result = buildCommand('opencode', 'test', {
      outputFormat: 'json',
      cliFeatures: { supportsJson: true },
    });

    assert.ok(result.args.includes('--format'));
    assert.ok(result.args.includes('json'));
  });

  it('includes model and variant when provided', function () {
    const result = buildCommand('opencode', 'test', {
      modelSpec: { model: 'opencode/glm-4.7-free', reasoningEffort: 'high' },
      cliFeatures: { supportsJson: true, supportsVariant: true },
    });

    assert.ok(result.args.includes('--model'));
    assert.ok(result.args.includes('opencode/glm-4.7-free'));
    assert.ok(result.args.includes('--variant'));
    assert.ok(result.args.includes('high'));
  });

  describe('worktree isolation via --dir flag', function () {
    it('passes --dir <cwd> when supportsDir is true', function () {
      const result = buildCommand('opencode', 'test prompt', {
        cwd: '/worktrees/spinning-cosmos-71',
        cliFeatures: { supportsDir: true, supportsCwd: false },
      });

      assert.ok(result.args.includes('--dir'), 'expected --dir flag in args');
      assert.ok(
        result.args.includes('/worktrees/spinning-cosmos-71'),
        'expected worktree path in args'
      );
      assert.ok(!result.args.includes('--cwd'), '--cwd must not be used when supportsDir is true');
    });

    it('falls back to --cwd when supportsCwd is true and supportsDir is false', function () {
      const result = buildCommand('opencode', 'test prompt', {
        cwd: '/worktrees/spinning-cosmos-71',
        cliFeatures: { supportsDir: false, supportsCwd: true },
      });

      assert.ok(result.args.includes('--cwd'), 'expected --cwd flag in args');
      assert.ok(!result.args.includes('--dir'), '--dir must not be used when supportsDir is false');
    });

    it('sets commandSpec.cwd even when neither --dir nor --cwd flag is supported', function () {
      const result = buildCommand('opencode', 'test prompt', {
        cwd: '/worktrees/spinning-cosmos-71',
        cliFeatures: { supportsDir: false, supportsCwd: false },
      });

      assert.strictEqual(
        result.cwd,
        '/worktrees/spinning-cosmos-71',
        'commandSpec.cwd must be set so the process spawns in the worktree'
      );
      assert.ok(!result.args.includes('--dir'));
      assert.ok(!result.args.includes('--cwd'));
    });

    it('detects supportsDir from help text containing --dir', function () {
      const { opencodeAdapter } = require('../lib/agent-cli-provider/adapters/opencode');
      const features = opencodeAdapter.detectCliFeatures(
        'Usage: opencode run [options]\n  --dir  Working directory\n  --model  Model to use\n'
      );

      assert.strictEqual(
        features.supportsDir,
        true,
        'supportsDir must be true when --dir appears in help text'
      );
      assert.strictEqual(
        features.supportsCwd,
        false,
        'supportsCwd must be false when --cwd is absent from help text'
      );
    });

    it('detects supportsDir as false from help text without --dir', function () {
      const { opencodeAdapter } = require('../lib/agent-cli-provider/adapters/opencode');
      const features = opencodeAdapter.detectCliFeatures(
        'Usage: opencode run [options]\n  --cwd  Working directory\n'
      );

      assert.strictEqual(
        features.supportsDir,
        false,
        'supportsDir must be false when --dir is absent from help text'
      );
      assert.strictEqual(
        features.supportsCwd,
        true,
        'supportsCwd must be true when --cwd appears in help text'
      );
    });
  });
});

describe('Claude provider helper builder', function () {
  const originalEnv = process.env.ZEROSHOT_CLAUDE_COMMAND;

  afterEach(function () {
    if (originalEnv === undefined) {
      delete process.env.ZEROSHOT_CLAUDE_COMMAND;
    } else {
      process.env.ZEROSHOT_CLAUDE_COMMAND = originalEnv;
    }
  });

  it('passes --json-schema when CLI supports it', function () {
    const schema = { type: 'object', properties: { foo: { type: 'string' } } };
    const result = buildCommand('claude', 'test context', {
      jsonSchema: schema,
      outputFormat: 'json',
      cliFeatures: { supportsJsonSchema: true },
    });

    assert.ok(result.args.includes('--json-schema'));
  });

  it('does not pass --json-schema when outputFormat is not json', function () {
    const schema = { type: 'object', properties: { foo: { type: 'string' } } };
    const result = buildCommand('claude', 'test context', {
      jsonSchema: schema,
      outputFormat: 'stream-json',
      cliFeatures: { supportsJsonSchema: true },
    });

    assert.ok(!result.args.includes('--json-schema'));
  });

  it('does not pass --json-schema when CLI does not support it', function () {
    const schema = { type: 'object', properties: { foo: { type: 'string' } } };
    const result = buildCommand('claude', 'test context', {
      jsonSchema: schema,
      outputFormat: 'json',
      cliFeatures: { supportsJsonSchema: false },
    });

    assert.ok(!result.args.includes('--json-schema'));
  });

  it('passes model value through without alias normalization', function () {
    const result = buildCommand('claude', 'test context', {
      modelSpec: { model: 'opus-4.6' },
      cliFeatures: { supportsModel: true },
    });

    const modelFlagIndex = result.args.indexOf('--model');
    assert.ok(modelFlagIndex >= 0);
    assert.strictEqual(result.args[modelFlagIndex + 1], 'opus-4.6');
  });

  it('adds Claude resume and continue args before the prompt', function () {
    const resumed = buildCommand('claude', 'test context', {
      resumeSessionId: 'session-123',
    });
    assert.deepStrictEqual(resumed.args.slice(-3), ['--resume', 'session-123', 'test context']);

    const continued = buildCommand('claude', 'test context', {
      continueSession: true,
    });
    assert.deepStrictEqual(continued.args.slice(-2), ['--continue', 'test context']);
  });

  it('honors configured Claude executable selection inside the helper', function () {
    process.env.ZEROSHOT_CLAUDE_COMMAND = 'ccr code';
    const result = buildCommand('claude', 'test context');

    assert.strictEqual(result.binary, 'ccr');
    assert.deepStrictEqual(result.args.slice(0, 3), ['code', '--print', '--input-format']);
  });
});

describe('Provider helper builder regressions', function () {
  it('Codex planner schema is injected when native schema support is unavailable', function () {
    const plannerSchema = {
      type: 'object',
      properties: {
        plan: { type: 'string' },
        summary: { type: 'string' },
        filesAffected: { type: 'array', items: { type: 'string' } },
      },
      required: ['plan', 'summary'],
    };

    const result = buildCommand('codex', 'Create a plan for implementing feature X', {
      jsonSchema: plannerSchema,
      outputFormat: 'json',
      cliFeatures: { supportsOutputSchema: false, supportsJson: true },
    });

    const finalContext = result.args[result.args.length - 1];

    assert.ok(finalContext.includes('OUTPUT FORMAT'));
    assert.ok(finalContext.includes('ONLY valid JSON'));
    assert.ok(finalContext.includes('"plan"'));
    assert.ok(finalContext.includes('"summary"'));
  });

  it('Gemini conductor schema is injected', function () {
    const conductorSchema = {
      type: 'object',
      properties: {
        complexity: { type: 'string', enum: ['TRIVIAL', 'SIMPLE', 'STANDARD', 'CRITICAL'] },
        taskType: { type: 'string', enum: ['INQUIRY', 'TASK', 'DEBUG'] },
        reasoning: { type: 'string' },
      },
      required: ['complexity', 'taskType', 'reasoning'],
    };

    const result = buildCommand('gemini', 'Classify this task', {
      jsonSchema: conductorSchema,
      outputFormat: 'json',
      cliFeatures: { supportsStreamJson: true },
    });

    const pIndex = result.args.indexOf('-p');
    const finalContext = result.args[pIndex + 1];

    assert.ok(finalContext.includes('OUTPUT FORMAT'));
    assert.ok(finalContext.includes('"complexity"'));
    assert.ok(finalContext.includes('"taskType"'));
  });
});
