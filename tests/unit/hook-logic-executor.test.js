/**
 * Hook Logic Executor Tests
 *
 * Tests for evaluateHookLogic and deepMerge functions in agent-hook-executor.js
 */

const assert = require('assert');
const { evaluateHookLogic, deepMerge } = require('../../src/agent/agent-hook-executor');

const mockAgent = {
  id: 'test-worker',
  role: 'implementation',
  iteration: 1,
  _log: () => {},
};

const mockContext = {
  triggeringMessage: { topic: 'TEST', content: { text: 'test' } },
};

describe('Hook Logic Executor', function () {
  registerDeepMergeTests();
  registerEvaluateHookLogicBaseTests();
  registerEvaluateHookLogicReturnTests();
  registerEvaluateHookLogicDataAccessTests();
  registerEvaluateHookLogicErrorTests();
});

function registerDeepMergeTests() {
  describe('deepMerge', function () {
    it('should merge simple objects', function () {
      const target = { a: 1, b: 2 };
      const source = { b: 3, c: 4 };
      const result = deepMerge(target, source);
      assert.deepStrictEqual(result, { a: 1, b: 3, c: 4 });
    });

    it('should deeply merge nested objects', function () {
      const target = { outer: { a: 1, b: 2 } };
      const source = { outer: { b: 3, c: 4 } };
      const result = deepMerge(target, source);
      assert.deepStrictEqual(result, { outer: { a: 1, b: 3, c: 4 } });
    });

    it('should replace arrays instead of merging', function () {
      const target = { arr: [1, 2, 3] };
      const source = { arr: [4, 5] };
      const result = deepMerge(target, source);
      assert.deepStrictEqual(result, { arr: [4, 5] });
    });

    it('should handle null source', function () {
      const target = { a: 1 };
      const result = deepMerge(target, null);
      assert.deepStrictEqual(result, { a: 1 });
    });

    it('should handle null target', function () {
      const source = { a: 1 };
      const result = deepMerge(null, source);
      assert.deepStrictEqual(result, { a: 1 });
    });

    it('should not mutate original objects', function () {
      const target = { a: 1 };
      const source = { b: 2 };
      deepMerge(target, source);
      assert.deepStrictEqual(target, { a: 1 });
      assert.deepStrictEqual(source, { b: 2 });
    });
  });
}

function registerEvaluateHookLogicBaseTests() {
  describe('evaluateHookLogic - base cases', function () {
    it('should return null for missing logic', function () {
      const result = evaluateHookLogic({
        logic: null,
        resultData: {},
        agent: mockAgent,
        context: mockContext,
      });
      assert.strictEqual(result, null);
    });

    it('should return null for logic without script', function () {
      const result = evaluateHookLogic({
        logic: { engine: 'javascript' },
        resultData: {},
        agent: mockAgent,
        context: mockContext,
      });
      assert.strictEqual(result, null);
    });

    it('should throw for non-javascript engine', function () {
      assert.throws(() => {
        evaluateHookLogic({
          logic: { engine: 'python', script: 'return True' },
          resultData: {},
          agent: mockAgent,
          context: mockContext,
        });
      }, /Unsupported hook logic engine/);
    });
  });
}

function registerEvaluateHookLogicReturnTests() {
  describe('evaluateHookLogic - return values', function () {
    it('should return config override when script returns object', function () {
      const result = evaluateHookLogic({
        logic: {
          engine: 'javascript',
          script: "return { topic: 'WORKER_PROGRESS' };",
        },
        resultData: { canValidate: false },
        agent: mockAgent,
        context: mockContext,
      });
      // Use deepEqual because VM sandbox returns objects with different prototype chain
      assert.deepEqual(result, { topic: 'WORKER_PROGRESS' });
    });

    it('should return null when script returns undefined', function () {
      const result = evaluateHookLogic({
        logic: {
          engine: 'javascript',
          script: 'if (false) return {};',
        },
        resultData: {},
        agent: mockAgent,
        context: mockContext,
      });
      assert.strictEqual(result, null);
    });

    it('should return null when script returns null', function () {
      const result = evaluateHookLogic({
        logic: {
          engine: 'javascript',
          script: 'return null;',
        },
        resultData: {},
        agent: mockAgent,
        context: mockContext,
      });
      assert.strictEqual(result, null);
    });

    it('should throw when script returns non-object', function () {
      assert.throws(() => {
        evaluateHookLogic({
          logic: {
            engine: 'javascript',
            script: "return 'string';",
          },
          resultData: {},
          agent: mockAgent,
          context: mockContext,
        });
      }, /must return an object or undefined/);
    });
  });
}

function registerEvaluateHookLogicDataAccessTests() {
  describe('evaluateHookLogic - data access', function () {
    it('should have access to result data', function () {
      const result = evaluateHookLogic({
        logic: {
          engine: 'javascript',
          script: "if (!result.completionStatus?.canValidate) return { topic: 'PROGRESS' };",
        },
        resultData: {
          completionStatus: { canValidate: false, percentComplete: 50 },
        },
        agent: mockAgent,
        context: mockContext,
      });
      assert.deepEqual(result, { topic: 'PROGRESS' });
    });

    it('should return null when condition not met', function () {
      const result = evaluateHookLogic({
        logic: {
          engine: 'javascript',
          script: "if (!result.completionStatus?.canValidate) return { topic: 'PROGRESS' };",
        },
        resultData: {
          completionStatus: { canValidate: true, percentComplete: 100 },
        },
        agent: mockAgent,
        context: mockContext,
      });
      assert.strictEqual(result, null);
    });

    it('should have access to agent info', function () {
      const result = evaluateHookLogic({
        logic: {
          engine: 'javascript',
          script: 'return { agentId: agent.id, iteration: agent.iteration };',
        },
        resultData: {},
        agent: mockAgent,
        context: mockContext,
      });
      assert.deepEqual(result, { agentId: 'test-worker', iteration: 1 });
    });
  });
}

function registerEvaluateHookLogicErrorTests() {
  describe('evaluateHookLogic - error handling', function () {
    it('should throw on script runtime error', function () {
      assert.throws(() => {
        evaluateHookLogic({
          logic: {
            engine: 'javascript',
            script: 'throw new Error("test error");',
          },
          resultData: {},
          agent: mockAgent,
          context: mockContext,
        });
      }, /Hook logic script error/);
    });

    it('should timeout on infinite loop', function () {
      assert.throws(() => {
        evaluateHookLogic({
          logic: {
            engine: 'javascript',
            script: 'while(true) {}',
          },
          resultData: {},
          agent: mockAgent,
          context: mockContext,
        });
      }, /timed out|Script execution/i);
    });
  });
}
