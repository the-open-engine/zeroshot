const assert = require('assert');
const MockTaskRunner = require('./helpers/mock-task-runner');

let mockRunner;

describe('MockTaskRunner - Model-specific behavior', () => {
  beforeEach(() => {
    mockRunner = new MockTaskRunner();
  });

  afterEach(() => {
    mockRunner.reset();
  });

  defineWithModelTests();
  defineOverrideModelDefaultsTests();
  defineModelAwareBehaviorTests();
  defineAssertCalledWithModelTests();
  defineModelDefaultsWithoutWithModelTests();
});

function defineWithModelTests() {
  describe('withModel() API', () => {
    it('should store expected model in behavior', () => {
      mockRunner.when('planner').withModel('opus').returns({ plan: 'detailed' });

      const behavior = mockRunner.behaviors.get('planner');
      assert.strictEqual(behavior.expectedModel, 'opus');
    });

    it('should validate model during run()', async function () {
      this.timeout(3000);
      mockRunner.when('planner').withModel('opus').returns({ plan: 'detailed' });

      // Should succeed with correct model
      const result = await mockRunner.run('context', {
        agentId: 'planner',
        model: 'opus',
      });
      assert.strictEqual(result.success, true);
    });

    it('should throw error when wrong model is used', async () => {
      mockRunner.when('planner').withModel('opus').returns({ plan: 'detailed' });

      await assert.rejects(
        async () => {
          await mockRunner.run('context', {
            agentId: 'planner',
            model: 'haiku',
          });
        },
        {
          name: 'Error',
          message:
            'Expected agent "planner" to be called with model "opus", but was called with "haiku"',
        }
      );
    });

    it('should not validate model when expectedModel is not set', async () => {
      mockRunner.when('worker').returns({ result: 'done' });

      // Should succeed with any model
      const result1 = await mockRunner.run('context', {
        agentId: 'worker',
        model: 'opus',
      });
      assert.strictEqual(result1.success, true);

      const result2 = await mockRunner.run('context', {
        agentId: 'worker',
        model: 'haiku',
      });
      assert.strictEqual(result2.success, true);
    });
  });
}

// DELETED: "Model-specific default delays" tests - too slow (~4.5s of real delays)
// Testing mock infrastructure delays has low value - if delays break we'd notice in usage

function defineOverrideModelDefaultsTests() {
  describe('Override model defaults', () => {
    it('should allow explicit delay to override model default', async () => {
      mockRunner.when('planner').withModel('opus').delays(100, { plan: 'quick' });

      const startTime = Date.now();
      await mockRunner.run('context', { agentId: 'planner', model: 'opus' });
      const duration = Date.now() - startTime;

      // Should use explicit 100ms, not opus default of 2000ms
      assert(duration >= 50 && duration < 250, `Expected ~100ms delay, got ${duration}ms`);
    });

    it('should still validate model even with explicit delay', async () => {
      mockRunner.when('planner').withModel('opus').delays(100, { plan: 'quick' });

      await assert.rejects(
        async () => {
          await mockRunner.run('context', {
            agentId: 'planner',
            model: 'haiku',
          });
        },
        {
          name: 'Error',
          message:
            'Expected agent "planner" to be called with model "opus", but was called with "haiku"',
        }
      );
    });
  });
}

function defineModelAwareBehaviorTests() {
  describe('Model-aware behaviors', () => {
    it('should work with fails()', async () => {
      mockRunner.when('worker').withModel('sonnet').fails('error message');

      const result = await mockRunner.run('context', {
        agentId: 'worker',
        model: 'sonnet',
      });
      assert.strictEqual(result.success, false);
      assert.strictEqual(result.error, 'error message');
    });

    it('should work with calls()', async () => {
      mockRunner
        .when('worker')
        .withModel('sonnet')
        .calls((context, options) => {
          return {
            success: true,
            output: `Custom output for ${options.model}`,
            error: null,
          };
        });

      const result = await mockRunner.run('context', {
        agentId: 'worker',
        model: 'sonnet',
      });
      assert.strictEqual(result.success, true);
      assert.strictEqual(result.output, 'Custom output for sonnet');
    });

    it('should work with streams()', async () => {
      const events = [
        { type: 'text_delta', text: 'Hello' },
        { type: 'text_delta', text: ' world' },
      ];

      mockRunner.when('worker').withModel('haiku').streams(events, 10).thenReturns({ done: true });

      const result = await mockRunner.run('context', {
        agentId: 'worker',
        model: 'haiku',
      });
      assert.strictEqual(result.success, true);
      assert.strictEqual(result.output, JSON.stringify({ done: true }));

      // Verify stream events were stored in call record
      const calls = mockRunner.getCalls('worker');
      assert.strictEqual(calls.length, 1);
      assert.strictEqual(calls[0].streamEvents.length, 2);
    });
  });
}

function defineAssertCalledWithModelTests() {
  describe('assertCalledWithModel()', () => {
    it('should pass when model matches', async () => {
      await mockRunner.run('context', { agentId: 'planner', model: 'opus' });

      // Should not throw
      mockRunner.assertCalledWithModel('planner', 'opus');
    });

    it('should fail when model does not match', async () => {
      await mockRunner.run('context', { agentId: 'planner', model: 'haiku' });

      assert.throws(
        () => {
          mockRunner.assertCalledWithModel('planner', 'opus');
        },
        (error) => {
          return (
            error.name === 'AssertionError' &&
            error.message.includes(
              'Expected agent "planner" to be called with model "opus", but was called with "haiku"'
            )
          );
        },
        'Expected assertion error for model mismatch'
      );
    });

    it('should fail when agent was never called', () => {
      assert.throws(
        () => {
          mockRunner.assertCalledWithModel('planner', 'opus');
        },
        {
          name: 'AssertionError',
          message: 'Expected agent "planner" to be called, but it was never called',
        }
      );
    });

    it('should check the last call when agent is called multiple times', async () => {
      await mockRunner.run('context', { agentId: 'worker', model: 'haiku' });
      await mockRunner.run('context', { agentId: 'worker', model: 'sonnet' });
      await mockRunner.run('context', { agentId: 'worker', model: 'opus' });

      // Should check last call (opus)
      mockRunner.assertCalledWithModel('worker', 'opus');

      // Should fail for earlier models
      assert.throws(
        () => {
          mockRunner.assertCalledWithModel('worker', 'haiku');
        },
        {
          name: 'AssertionError',
        }
      );
    });
  });
}

// DELETED: "Real-world use case: Model escalation" test - too slow (~2.5s of real delays)
// DELETED: "Real-world use case: Parallel validators" test - too slow (~3.5s of real delays)
// Testing mock infrastructure delays has low value - if delays break we'd notice in usage

function defineModelDefaultsWithoutWithModelTests() {
  describe('Model defaults without withModel()', () => {
    it('should not apply default delays when withModel() is not used', async () => {
      mockRunner.when('agent').returns({ result: 'instant' });

      const startTime = Date.now();
      await mockRunner.run('context', { agentId: 'agent', model: 'opus' });
      const duration = Date.now() - startTime;

      // Should be instant (no delay)
      assert(duration < 50, `Expected instant response, got ${duration}ms`);
    });

    it('should still allow explicit delays without withModel()', async () => {
      mockRunner.when('agent').delays(200, { result: 'delayed' });

      const startTime = Date.now();
      await mockRunner.run('context', { agentId: 'agent', model: 'opus' });
      const duration = Date.now() - startTime;

      // Should use explicit delay
      assert(duration >= 150 && duration < 350, `Expected ~200ms delay, got ${duration}ms`);
    });
  });
}
