/**
 * Tests for MockTaskRunner assertion API
 *
 * Verifies that all assertion methods work correctly and provide clear error messages.
 */

const assert = require('node:assert');
const MockTaskRunner = require('./helpers/mock-task-runner');

let mockRunner;

describe('MockTaskRunner Assertion API', () => {
  beforeEach(() => {
    mockRunner = new MockTaskRunner();
  });

  defineAssertCalledWithModelTests();
  defineAssertCalledWithOutputFormatTests();
  defineAssertCalledWithJsonSchemaTests();
  defineAssertContextIncludesTests();
  defineAssertContextExcludesTests();
  defineAssertCalledWithOptionsTests();
  defineComplexCallPatternTests();
  defineErrorMessageQualityTests();
});

function defineAssertCalledWithModelTests() {
  describe('assertCalledWithModel', () => {
    it('should pass when agent called with correct model', async () => {
      mockRunner.when('worker').returns('{}');

      await mockRunner.run('test context', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      mockRunner.assertCalledWithModel('worker', 'sonnet');
    });

    it('should fail with clear error when model does not match', () => {
      mockRunner.when('worker').returns({ result: 'done' });

      mockRunner.run('context', { agentId: 'worker', model: 'opus' });

      try {
        mockRunner.assertCalledWithModel('worker', 'sonnet');
        assert.fail('Should have thrown');
      } catch (err) {
        // Test behavior: error mentions both expected and actual models
        assert(err.message.includes('sonnet'), 'Error should mention expected model');
        assert(err.message.includes('opus'), 'Error should mention actual model');
      }
    });

    it('should fail when agent was never called', () => {
      assert.throws(
        () => mockRunner.assertCalledWithModel('worker', 'sonnet'),
        /Expected agent "worker" to be called, but it was never called/
      );
    });

    it('should handle multiple calls with different models', async () => {
      mockRunner.when('worker').returns('{}');

      await mockRunner.run('test 1', { agentId: 'worker', model: 'opus' });
      await mockRunner.run('test 2', { agentId: 'worker', modelLevel: 'level2' });

      mockRunner.assertCalledWithModel('worker', 'sonnet');
    });
  });
}

function defineAssertCalledWithOutputFormatTests() {
  describe('assertCalledWithOutputFormat', () => {
    it('should pass when agent called with correct output format', async () => {
      mockRunner.when('validator').returns('{"approved": true}');

      await mockRunner.run('test context', {
        agentId: 'validator',
        outputFormat: 'json',
      });

      mockRunner.assertCalledWithOutputFormat('validator', 'json');
    });

    it('should fail with clear error when output format does not match', async () => {
      mockRunner.when('validator').returns('{}');

      await mockRunner.run('test context', {
        agentId: 'validator',
        outputFormat: 'text',
      });

      assert.throws(
        () => mockRunner.assertCalledWithOutputFormat('validator', 'json'),
        /Expected agent "validator" to be called with outputFormat "json", but found: text/
      );
    });

    it('should handle undefined output format', async () => {
      mockRunner.when('worker').returns('{}');

      await mockRunner.run('test context', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      assert.throws(
        () => mockRunner.assertCalledWithOutputFormat('worker', 'json'),
        /but found: undefined/
      );
    });
  });
}

function defineAssertCalledWithJsonSchemaTests() {
  describe('assertCalledWithJsonSchema', () => {
    const testSchema = {
      type: 'object',
      properties: {
        approved: { type: 'boolean' },
        issues: { type: 'array' },
      },
      required: ['approved'],
    };

    it('should pass when agent called with matching JSON schema', async () => {
      mockRunner.when('validator').returns('{}');

      await mockRunner.run('test context', {
        agentId: 'validator',
        jsonSchema: testSchema,
      });

      mockRunner.assertCalledWithJsonSchema('validator', testSchema);
    });

    it('should fail when schema does not match', async () => {
      mockRunner.when('validator').returns('{}');

      const differentSchema = {
        type: 'object',
        properties: { result: { type: 'string' } },
      };

      await mockRunner.run('test context', {
        agentId: 'validator',
        jsonSchema: differentSchema,
      });

      assert.throws(
        () => mockRunner.assertCalledWithJsonSchema('validator', testSchema),
        /Expected agent "validator" to be called with specific JSON schema, but none matched/
      );
    });

    it('should fail when no JSON schema provided', async () => {
      mockRunner.when('validator').returns('{}');

      await mockRunner.run('test context', {
        agentId: 'validator',
        modelLevel: 'level2',
      });

      assert.throws(
        () => mockRunner.assertCalledWithJsonSchema('validator', testSchema),
        /but none matched/
      );
    });
  });
}

function defineAssertContextIncludesTests() {
  describe('assertContextIncludes', () => {
    it('should pass when context includes expected substring', async () => {
      mockRunner.when('worker').returns('{}');

      await mockRunner.run('PLAN_READY message from planner', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      mockRunner.assertContextIncludes('worker', 'PLAN_READY message');
    });

    it('should fail with clear error when substring not found', async () => {
      mockRunner.when('worker').returns('{}');

      await mockRunner.run('some other context', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      assert.throws(
        () => mockRunner.assertContextIncludes('worker', 'PLAN_READY'),
        /Expected agent "worker" context to include "PLAN_READY", but no calls matched/
      );
    });

    it('should work with multiple calls', async () => {
      mockRunner.when('worker').returns('{}');

      await mockRunner.run('first context', {
        agentId: 'worker',
        modelLevel: 'level2',
      });
      await mockRunner.run('second context with PLAN_READY', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      mockRunner.assertContextIncludes('worker', 'PLAN_READY');
    });
  });
}

function defineAssertContextExcludesTests() {
  describe('assertContextExcludes', () => {
    it('should pass when context does not include unwanted substring', async () => {
      mockRunner.when('worker').returns('{}');

      await mockRunner.run('clean context without secrets', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      mockRunner.assertContextExcludes('worker', 'API_KEY');
    });

    it('should fail when unwanted substring is found', async () => {
      mockRunner.when('worker').returns('{}');

      await mockRunner.run('context with API_KEY=secret', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      assert.throws(
        () => mockRunner.assertContextExcludes('worker', 'API_KEY'),
        /Expected agent "worker" context to exclude "API_KEY", but found it in 1 call\(s\)/
      );
    });

    it('should count multiple occurrences', async () => {
      mockRunner.when('worker').returns('{}');

      await mockRunner.run('first with SECRET', {
        agentId: 'worker',
        modelLevel: 'level2',
      });
      await mockRunner.run('second with SECRET', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      assert.throws(
        () => mockRunner.assertContextExcludes('worker', 'SECRET'),
        /but found it in 2 call\(s\)/
      );
    });
  });
}

function defineAssertCalledWithOptionsTests() {
  describe('assertCalledWithOptions', () => {
    it('should pass when all specified options match', async () => {
      mockRunner.when('validator').returns('{}');

      await mockRunner.run('test context', {
        agentId: 'validator',
        modelLevel: 'level2',
        outputFormat: 'json',
        cwd: '/workspace',
      });

      mockRunner.assertCalledWithOptions('validator', {
        modelLevel: 'level2',
        outputFormat: 'json',
      });
    });

    it('should support partial matching', async () => {
      mockRunner.when('validator').returns('{}');

      await mockRunner.run('test context', {
        agentId: 'validator',
        modelLevel: 'level2',
        outputFormat: 'json',
        cwd: '/workspace',
        isolation: true,
      });

      mockRunner.assertCalledWithOptions('validator', {
        modelLevel: 'level2',
      });
    });

    it('should fail when options do not match', async () => {
      mockRunner.when('validator').returns('{}');

      await mockRunner.run('test context', {
        agentId: 'validator',
        model: 'opus',
        outputFormat: 'text',
      });

      assert.throws(
        () =>
          mockRunner.assertCalledWithOptions('validator', {
            modelLevel: 'level2',
            outputFormat: 'json',
          }),
        /Expected agent "validator" to be called with options.*but no calls matched/
      );
    });

    it('should support object matching with deep equality', async () => {
      mockRunner.when('validator').returns('{}');

      const schema = {
        type: 'object',
        properties: { test: { type: 'string' } },
      };

      await mockRunner.run('test context', {
        agentId: 'validator',
        jsonSchema: schema,
      });

      mockRunner.assertCalledWithOptions('validator', {
        jsonSchema: schema,
      });
    });

    it('should fail when object values do not match', async () => {
      mockRunner.when('validator').returns('{}');

      const schema1 = {
        type: 'object',
        properties: { test: { type: 'string' } },
      };
      const schema2 = {
        type: 'object',
        properties: { test: { type: 'number' } },
      };

      await mockRunner.run('test context', {
        agentId: 'validator',
        jsonSchema: schema1,
      });

      assert.throws(
        () =>
          mockRunner.assertCalledWithOptions('validator', {
            jsonSchema: schema2,
          }),
        /but no calls matched/
      );
    });
  });
}

function defineComplexCallPatternTests() {
  describe('Complex call patterns', () => {
    it('should verify model escalation workflow', async () => {
      mockRunner.when('worker').returns('{}');

      // First call with sonnet
      await mockRunner.run('initial attempt', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      // Second call with opus (escalated)
      await mockRunner.run('retry with better model', {
        agentId: 'worker',
        model: 'opus',
      });

      const calls = mockRunner.getCalls('worker');
      assert.strictEqual(calls.length, 2);
      assert.strictEqual(calls[0].options.model, 'sonnet');
      assert.strictEqual(calls[1].options.model, 'opus');

      mockRunner.assertCalledWithModel('worker', 'opus');
    });

    it('should verify validation workflow with context and format', async () => {
      mockRunner.when('validator').returns('{"approved": true}');

      await mockRunner.run('Review IMPLEMENTATION_READY from worker', {
        agentId: 'validator',
        modelLevel: 'level2',
        outputFormat: 'json',
        jsonSchema: {
          type: 'object',
          properties: { approved: { type: 'boolean' } },
        },
      });

      mockRunner.assertContextIncludes('validator', 'IMPLEMENTATION_READY');
      mockRunner.assertCalledWithOutputFormat('validator', 'json');
      mockRunner.assertCalledWithOptions('validator', {
        modelLevel: 'level2',
        outputFormat: 'json',
      });
    });

    it('should verify rejection loop context evolution', async () => {
      mockRunner.when('worker').returns('{}');

      await mockRunner.run('Initial context', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      await mockRunner.run('Context with VALIDATION_RESULT: rejected', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      await mockRunner.run('Context with VALIDATION_RESULT: approved', {
        agentId: 'worker',
        modelLevel: 'level2',
      });

      const calls = mockRunner.getCalls('worker');
      assert.strictEqual(calls.length, 3);
      assert(!calls[0].context.includes('VALIDATION_RESULT'));
      assert(calls[1].context.includes('rejected'));
      assert(calls[2].context.includes('approved'));
    });
  });
}

function defineErrorMessageQualityTests() {
  describe('Error message quality', () => {
    it('should provide helpful error when agent not called', () => {
      assert.throws(
        () => mockRunner.assertCalledWithModel('nonexistent', 'sonnet'),
        /Expected agent "nonexistent" to be called, but it was never called/
      );
    });

    it('should show found values when assertion fails', async () => {
      mockRunner.when('planner').returns({ plan: 'test' });

      await mockRunner.run('context', { agentId: 'planner', model: 'opus' });

      try {
        mockRunner.assertCalledWithModel('planner', 'haiku');
        assert.fail('Should have thrown');
      } catch (err) {
        // Test behavior: error contains both expected and actual model
        assert(err.message.includes('haiku'), 'Error should mention expected model');
        assert(err.message.includes('opus'), 'Error should mention actual model');
      }
    });
  });
}
