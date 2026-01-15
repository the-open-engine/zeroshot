/**
 * Tests for MockTaskRunner output format validation
 */

const assert = require('assert');
const MockTaskRunner = require('./helpers/mock-task-runner');

let mockRunner;

describe('MockTaskRunner - Output Format Validation', function () {
  beforeEach(() => {
    mockRunner = new MockTaskRunner();
  });

  defineWithOutputFormatTests();
  defineWithJsonSchemaTests();
  defineSchemaValidationTests();
  defineRealWorldValidationTests();
  defineValidationWithOtherBehaviorTests();
  defineComplexNestedSchemaTests();
});

function defineWithOutputFormatTests() {
  describe('withOutputFormat', function () {
    it('should accept valid output formats', function () {
      // Should not throw
      mockRunner.when('agent1').withOutputFormat('text').returns('OK');
      mockRunner.when('agent2').withOutputFormat('json').returns({ status: 'OK' });
      mockRunner.when('agent3').withOutputFormat('stream-json').returns({ status: 'OK' });
    });

    it('should reject invalid output formats', function () {
      assert.throws(
        () => mockRunner.when('agent').withOutputFormat('xml'),
        /Invalid output format: xml/
      );

      assert.throws(
        () => mockRunner.when('agent').withOutputFormat('yaml'),
        /Invalid output format: yaml/
      );
    });

    it('should allow chaining with returns', function () {
      mockRunner.when('agent').withOutputFormat('json').returns({ status: 'success' });

      const behavior = mockRunner.behaviors.get('agent');
      assert.strictEqual(behavior.outputFormat, 'json');
      assert.strictEqual(behavior.type, 'success');
    });
  });
}

function defineWithJsonSchemaTests() {
  describe('withJsonSchema', function () {
    it('should accept valid JSON schema', function () {
      const schema = {
        type: 'object',
        properties: {
          approved: { type: 'boolean' },
        },
        required: ['approved'],
      };

      mockRunner.when('validator').withJsonSchema(schema).returns({ approved: true });

      const behavior = mockRunner.behaviors.get('validator');
      assert.deepStrictEqual(behavior.jsonSchema, schema);
    });

    it('should reject non-object schemas', function () {
      assert.throws(
        () => mockRunner.when('agent').withJsonSchema('not an object'),
        /JSON schema must be an object/
      );

      assert.throws(
        () => mockRunner.when('agent').withJsonSchema(null),
        /JSON schema must be an object/
      );
    });

    it('should allow chaining with withOutputFormat and returns', function () {
      const schema = {
        type: 'object',
        properties: { status: { type: 'string' } },
      };

      mockRunner
        .when('agent')
        .withOutputFormat('json')
        .withJsonSchema(schema)
        .returns({ status: 'OK' });

      const behavior = mockRunner.behaviors.get('agent');
      assert.strictEqual(behavior.outputFormat, 'json');
      assert.deepStrictEqual(behavior.jsonSchema, schema);
    });
  });
}

function defineSchemaValidationTests() {
  describe('Schema validation on run', function () {
    defineSchemaValidationSuccessTests();
    defineSchemaValidationFailureTests();
    defineSchemaValidationSkipTests();
    defineSchemaValidationStreamJsonTests();
  });
}

function defineSchemaValidationSuccessTests() {
  it('should pass validation when output matches schema', async function () {
    const schema = {
      type: 'object',
      properties: {
        approved: { type: 'boolean' },
        message: { type: 'string' },
      },
      required: ['approved'],
    };

    mockRunner
      .when('validator')
      .withOutputFormat('json')
      .withJsonSchema(schema)
      .returns({ approved: true, message: 'Looks good' });

    const result = await mockRunner.run('test context', {
      agentId: 'validator',
      modelLevel: 'level2',
    });

    assert.strictEqual(result.success, true);
    assert.strictEqual(result.error, null);
  });
}

function defineSchemaValidationFailureTests() {
  it('should fail validation when output does not match schema', async function () {
    const schema = {
      type: 'object',
      properties: {
        approved: { type: 'boolean' },
      },
      required: ['approved'],
    };

    // Wrong type - string instead of boolean
    mockRunner
      .when('validator')
      .withOutputFormat('json')
      .withJsonSchema(schema)
      .returns({ approved: 'yes' });

    const result = await mockRunner.run('test context', {
      agentId: 'validator',
      modelLevel: 'level2',
    });

    assert.strictEqual(result.success, false);
    assert(result.error.includes('Output validation failed'));
    assert(result.error.includes('approved'));
    assert(result.error.includes('must be boolean'));
  });

  it('should fail validation when required field is missing', async function () {
    const schema = {
      type: 'object',
      properties: {
        approved: { type: 'boolean' },
        reason: { type: 'string' },
      },
      required: ['approved', 'reason'],
    };

    // Missing 'reason' field
    mockRunner
      .when('validator')
      .withOutputFormat('json')
      .withJsonSchema(schema)
      .returns({ approved: true });

    const result = await mockRunner.run('test context', {
      agentId: 'validator',
      modelLevel: 'level2',
    });

    assert.strictEqual(result.success, false);
    assert(result.error.includes('Output validation failed'));
    assert(result.error.includes('must have required property'));
  });

  it('should provide clear error messages for multiple validation errors', async function () {
    const schema = {
      type: 'object',
      properties: {
        count: { type: 'number' },
        status: { type: 'string' },
        active: { type: 'boolean' },
      },
      required: ['count', 'status', 'active'],
    };

    // Multiple wrong types
    mockRunner
      .when('validator')
      .withOutputFormat('json')
      .withJsonSchema(schema)
      .returns({ count: 'five', status: 123, active: 'true' });

    const result = await mockRunner.run('test context', {
      agentId: 'validator',
      modelLevel: 'level2',
    });

    assert.strictEqual(result.success, false);
    assert(result.error.includes('count'));
    assert(result.error.includes('status'));
    assert(result.error.includes('active'));
  });

  it('should fail validation when output is not valid JSON', async function () {
    const schema = {
      type: 'object',
      properties: { approved: { type: 'boolean' } },
    };

    mockRunner
      .when('validator')
      .withOutputFormat('json')
      .withJsonSchema(schema)
      .returns('Not valid JSON {');

    const result = await mockRunner.run('test context', {
      agentId: 'validator',
      modelLevel: 'level2',
    });

    assert.strictEqual(result.success, false);
    assert(result.error.includes('Output is not valid JSON'));
  });
}

function defineSchemaValidationSkipTests() {
  it('should skip validation when no schema is configured', async function () {
    mockRunner.when('agent').withOutputFormat('json').returns({ anything: 'goes' });

    const result = await mockRunner.run('test context', {
      agentId: 'agent',
      modelLevel: 'level2',
    });

    assert.strictEqual(result.success, true);
    assert.strictEqual(result.error, null);
  });

  it('should skip validation when output format is text', async function () {
    const schema = {
      type: 'object',
      properties: { approved: { type: 'boolean' } },
    };

    // Format is 'text', so schema should be ignored
    mockRunner
      .when('agent')
      .withOutputFormat('text')
      .withJsonSchema(schema)
      .returns('This is just text, not JSON');

    const result = await mockRunner.run('test context', {
      agentId: 'agent',
      modelLevel: 'level2',
    });

    assert.strictEqual(result.success, true);
    assert.strictEqual(result.error, null);
  });

  it('should skip validation when no output format is set', async function () {
    const schema = {
      type: 'object',
      properties: { approved: { type: 'boolean' } },
    };

    // No output format specified
    mockRunner.when('agent').withJsonSchema(schema).returns('Plain text');

    const result = await mockRunner.run('test context', {
      agentId: 'agent',
      modelLevel: 'level2',
    });

    assert.strictEqual(result.success, true);
    assert.strictEqual(result.error, null);
  });
}

function defineSchemaValidationStreamJsonTests() {
  it('should validate with stream-json format', async function () {
    const schema = {
      type: 'object',
      properties: {
        approved: { type: 'boolean' },
      },
      required: ['approved'],
    };

    // Valid output with stream-json format
    mockRunner
      .when('agent')
      .withOutputFormat('stream-json')
      .withJsonSchema(schema)
      .returns({ approved: true });

    const result = await mockRunner.run('test context', {
      agentId: 'agent',
      modelLevel: 'level2',
    });

    assert.strictEqual(result.success, true);
    assert.strictEqual(result.error, null);
  });
}

function defineRealWorldValidationTests() {
  describe('Real-world validation scenarios', function () {
    it('should validate validator agent output with approval schema', async function () {
      const approvalSchema = {
        type: 'object',
        properties: {
          approved: { type: 'boolean' },
          issues: {
            type: 'array',
            items: { type: 'string' },
          },
          severity: {
            type: 'string',
            enum: ['low', 'medium', 'high', 'critical'],
          },
        },
        required: ['approved'],
      };

      // Valid approval
      mockRunner.when('validator').withOutputFormat('json').withJsonSchema(approvalSchema).returns({
        approved: true,
        issues: [],
        severity: 'low',
      });

      const result = await mockRunner.run('validate code', {
        agentId: 'validator',
        modelLevel: 'level2',
      });

      assert.strictEqual(result.success, true);
    });

    it('should fail validation with invalid severity enum', async function () {
      const approvalSchema = {
        type: 'object',
        properties: {
          approved: { type: 'boolean' },
          severity: {
            type: 'string',
            enum: ['low', 'medium', 'high', 'critical'],
          },
        },
        required: ['approved', 'severity'],
      };

      // Invalid severity value
      mockRunner.when('validator').withOutputFormat('json').withJsonSchema(approvalSchema).returns({
        approved: false,
        severity: 'super-critical', // Not in enum
      });

      const result = await mockRunner.run('validate code', {
        agentId: 'validator',
        modelLevel: 'level2',
      });

      assert.strictEqual(result.success, false);
      assert(result.error.includes('severity'));
      assert(result.error.includes('severity'), 'Error should mention the failing field');
    });

    it('should support per-agent schema configuration', async function () {
      // Different schemas for different agents
      const plannerSchema = {
        type: 'object',
        properties: {
          steps: { type: 'array', items: { type: 'string' } },
        },
        required: ['steps'],
      };

      const validatorSchema = {
        type: 'object',
        properties: {
          approved: { type: 'boolean' },
        },
        required: ['approved'],
      };

      mockRunner
        .when('planner')
        .withOutputFormat('json')
        .withJsonSchema(plannerSchema)
        .returns({ steps: ['Step 1', 'Step 2'] });

      mockRunner
        .when('validator')
        .withOutputFormat('json')
        .withJsonSchema(validatorSchema)
        .returns({ approved: true });

      // Planner should validate against planner schema
      const plannerResult = await mockRunner.run('create plan', {
        agentId: 'planner',
        model: 'opus',
      });
      assert.strictEqual(plannerResult.success, true);

      // Validator should validate against validator schema
      const validatorResult = await mockRunner.run('validate plan', {
        agentId: 'validator',
        modelLevel: 'level2',
      });
      assert.strictEqual(validatorResult.success, true);
    });

    it('should fail when planner returns wrong schema shape', async function () {
      const plannerSchema = {
        type: 'object',
        properties: {
          steps: {
            type: 'array',
            items: {
              type: 'object',
              properties: {
                action: { type: 'string' },
                file: { type: 'string' },
              },
              required: ['action', 'file'],
            },
          },
        },
        required: ['steps'],
      };

      // Returns array of strings instead of array of objects
      mockRunner
        .when('planner')
        .withOutputFormat('json')
        .withJsonSchema(plannerSchema)
        .returns({ steps: ['Step 1', 'Step 2'] });

      const result = await mockRunner.run('create plan', {
        agentId: 'planner',
        model: 'opus',
      });

      assert.strictEqual(result.success, false);
      assert(result.error.includes('must be object'));
    });
  });
}

function defineValidationWithOtherBehaviorTests() {
  describe('Validation with other behavior types', function () {
    it('should validate delayed responses', async function () {
      const schema = {
        type: 'object',
        properties: { status: { type: 'string' } },
        required: ['status'],
      };

      mockRunner
        .when('agent')
        .withOutputFormat('json')
        .withJsonSchema(schema)
        .delays(10, { status: 'complete' });

      const result = await mockRunner.run('test', {
        agentId: 'agent',
        modelLevel: 'level2',
      });

      assert.strictEqual(result.success, true);
    });

    it('should fail validation for delayed responses with wrong schema', async function () {
      const schema = {
        type: 'object',
        properties: { count: { type: 'number' } },
        required: ['count'],
      };

      mockRunner
        .when('agent')
        .withOutputFormat('json')
        .withJsonSchema(schema)
        .delays(10, { count: 'five' }); // Wrong type

      const result = await mockRunner.run('test', {
        agentId: 'agent',
        modelLevel: 'level2',
      });

      assert.strictEqual(result.success, false);
      assert(result.error.includes('must be number'));
    });

    it('should not validate error responses', async function () {
      const schema = {
        type: 'object',
        properties: { approved: { type: 'boolean' } },
      };

      mockRunner
        .when('agent')
        .withOutputFormat('json')
        .withJsonSchema(schema)
        .fails('Something went wrong');

      const result = await mockRunner.run('test', {
        agentId: 'agent',
        modelLevel: 'level2',
      });

      // Error responses should not be validated
      assert.strictEqual(result.success, false);
      assert.strictEqual(result.error, 'Something went wrong');
    });

    it('should validate streaming responses', async function () {
      const schema = {
        type: 'object',
        properties: { status: { type: 'string' } },
        required: ['status'],
      };

      const events = [{ type: 'text', text: 'Processing...' }, { type: 'complete' }];

      mockRunner
        .when('agent')
        .withOutputFormat('stream-json')
        .withJsonSchema(schema)
        .streams(events, 10)
        .thenReturns({ status: 'done' });

      const result = await mockRunner.run('test', {
        agentId: 'agent',
        modelLevel: 'level2',
      });

      assert.strictEqual(result.success, true);
    });

    it('should fail validation for streaming with wrong final output', async function () {
      const schema = {
        type: 'object',
        properties: { count: { type: 'number' } },
        required: ['count'],
      };

      const events = [{ type: 'text', text: 'Calculating...' }];

      mockRunner
        .when('agent')
        .withOutputFormat('stream-json')
        .withJsonSchema(schema)
        .streams(events, 10)
        .thenReturns({ count: 'invalid' }); // Wrong type

      const result = await mockRunner.run('test', {
        agentId: 'agent',
        modelLevel: 'level2',
      });

      assert.strictEqual(result.success, false);
      assert(result.error.includes('must be number'));
    });
  });
}

function defineComplexNestedSchemaTests() {
  describe('Complex nested schemas', function () {
    it('should validate deeply nested object structures', async function () {
      const schema = {
        type: 'object',
        properties: {
          metadata: {
            type: 'object',
            properties: {
              author: { type: 'string' },
              timestamp: { type: 'number' },
            },
            required: ['author', 'timestamp'],
          },
          results: {
            type: 'array',
            items: {
              type: 'object',
              properties: {
                test: { type: 'string' },
                passed: { type: 'boolean' },
              },
              required: ['test', 'passed'],
            },
          },
        },
        required: ['metadata', 'results'],
      };

      mockRunner
        .when('tester')
        .withOutputFormat('json')
        .withJsonSchema(schema)
        .returns({
          metadata: {
            author: 'test-runner',
            timestamp: Date.now(),
          },
          results: [
            { test: 'unit-test-1', passed: true },
            { test: 'unit-test-2', passed: false },
          ],
        });

      const result = await mockRunner.run('run tests', {
        agentId: 'tester',
        modelLevel: 'level2',
      });

      assert.strictEqual(result.success, true);
    });

    it('should fail validation for nested structure errors', async function () {
      const schema = {
        type: 'object',
        properties: {
          config: {
            type: 'object',
            properties: {
              timeout: { type: 'number' },
              retries: { type: 'number' },
            },
            required: ['timeout', 'retries'],
          },
        },
        required: ['config'],
      };

      // Wrong type in nested field
      mockRunner
        .when('agent')
        .withOutputFormat('json')
        .withJsonSchema(schema)
        .returns({
          config: {
            timeout: '5000', // Should be number
            retries: 3,
          },
        });

      const result = await mockRunner.run('test', {
        agentId: 'agent',
        modelLevel: 'level2',
      });

      assert.strictEqual(result.success, false);
      assert(result.error.includes('timeout'));
      assert(result.error.includes('must be number'));
    });
  });
}
