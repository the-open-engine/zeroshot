const assert = require('assert');
const AgentWrapper = require('../src/agent-wrapper');

// Mock message bus and cluster
const mockMessageBus = { publish: () => {}, subscribe: () => {} };
const mockCluster = { id: 'test-cluster', agents: [] };

describe('Model Selection', () => {
  describe('Static model (backward compatibility)', () => {
    it('should use static model when no rules provided', () => {
      const agent = new AgentWrapper({ id: 'test', model: 'opus', timeout: 0 }, mockMessageBus, mockCluster, {
        testMode: true,
        mockSpawnFn: () => {},
      });

      assert.strictEqual(agent._selectModel(), 'opus');
    });

    it('should default to sonnet if no model specified', () => {
      const agent = new AgentWrapper({ id: 'test', timeout: 0 }, mockMessageBus, mockCluster, {
        testMode: true,
        mockSpawnFn: () => {},
      });

      assert.strictEqual(agent._selectModel(), 'sonnet');
    });
  });

  describe('Dynamic model rules', () => {
    it('should match exact iteration', () => {
      const agent = new AgentWrapper(
        {
          id: 'test',
          timeout: 0,
          modelRules: [
            { iterations: '1', model: 'haiku' },
            { iterations: '2', model: 'sonnet' },
            { iterations: '3+', model: 'opus' },
          ],
        },
        mockMessageBus,
        mockCluster,
        { testMode: true, mockSpawnFn: () => {} }
      );

      agent.iteration = 1;
      assert.strictEqual(agent._selectModel(), 'haiku');

      agent.iteration = 2;
      assert.strictEqual(agent._selectModel(), 'sonnet');
    });

    it('should match range', () => {
      const agent = new AgentWrapper(
        {
          id: 'test',
          timeout: 0,
          modelRules: [
            { iterations: '1-3', model: 'sonnet' },
            { iterations: 'all', model: 'opus' },
          ],
        },
        mockMessageBus,
        mockCluster,
        { testMode: true, mockSpawnFn: () => {} }
      );

      agent.iteration = 1;
      assert.strictEqual(agent._selectModel(), 'sonnet');

      agent.iteration = 3;
      assert.strictEqual(agent._selectModel(), 'sonnet');

      agent.iteration = 4;
      assert.strictEqual(agent._selectModel(), 'opus');
    });

    it('should match open-ended range', () => {
      const agent = new AgentWrapper(
        {
          id: 'test',
          timeout: 0,
          modelRules: [
            { iterations: '1', model: 'sonnet' },
            { iterations: '2+', model: 'opus' },
          ],
        },
        mockMessageBus,
        mockCluster,
        { testMode: true, mockSpawnFn: () => {} }
      );

      agent.iteration = 1;
      assert.strictEqual(agent._selectModel(), 'sonnet');

      agent.iteration = 2;
      assert.strictEqual(agent._selectModel(), 'opus');

      agent.iteration = 5;
      assert.strictEqual(agent._selectModel(), 'opus');

      agent.iteration = 100;
      assert.strictEqual(agent._selectModel(), 'opus');
    });

    it('should use catch-all as fallback', () => {
      const agent = new AgentWrapper(
        {
          id: 'test',
          timeout: 0,
          modelRules: [
            { iterations: '1', model: 'haiku' },
            { iterations: 'all', model: 'sonnet' },
          ],
        },
        mockMessageBus,
        mockCluster,
        { testMode: true, mockSpawnFn: () => {} }
      );

      agent.iteration = 999;
      assert.strictEqual(agent._selectModel(), 'sonnet');
    });

    it('should use first matching rule', () => {
      const agent = new AgentWrapper(
        {
          id: 'test',
          timeout: 0,
          modelRules: [
            { iterations: '1-10', model: 'sonnet' },
            { iterations: '5+', model: 'opus' }, // Overlaps but shouldn't match
          ],
        },
        mockMessageBus,
        mockCluster,
        { testMode: true, mockSpawnFn: () => {} }
      );

      agent.iteration = 7;
      assert.strictEqual(agent._selectModel(), 'sonnet'); // First rule wins
    });
  });

  describe('Error handling', () => {
    it('should throw if no rules match', () => {
      const agent = new AgentWrapper(
        {
          id: 'test',
          timeout: 0,
          modelRules: [{ iterations: '1-2', model: 'sonnet' }],
        },
        mockMessageBus,
        mockCluster,
        { testMode: true, mockSpawnFn: () => {} }
      );

      agent.iteration = 5;
      assert.throws(() => agent._selectModel(), /No model rule matched iteration 5/);
    });

    it('should throw on invalid pattern syntax', () => {
      const agent = new AgentWrapper(
        {
          id: 'test',
          timeout: 0,
          modelRules: [{ iterations: 'invalid', model: 'sonnet' }],
        },
        mockMessageBus,
        mockCluster,
        { testMode: true, mockSpawnFn: () => {} }
      );

      agent.iteration = 1;
      assert.throws(() => agent._selectModel(), /Invalid iteration pattern 'invalid'/);
    });
  });

  describe('Real-world use case', () => {
    it('should escalate from sonnet to opus on second iteration', () => {
      const agent = new AgentWrapper(
        {
          id: 'worker',
          timeout: 0,
          modelRules: [
            { iterations: '1', model: 'sonnet' },
            { iterations: '2+', model: 'opus' },
          ],
        },
        mockMessageBus,
        mockCluster,
        { testMode: true, mockSpawnFn: () => {} }
      );

      agent.iteration = 1;
      assert.strictEqual(agent._selectModel(), 'sonnet', 'First iteration should use sonnet');

      agent.iteration = 2;
      assert.strictEqual(agent._selectModel(), 'opus', 'Second iteration should escalate to opus');

      agent.iteration = 3;
      assert.strictEqual(agent._selectModel(), 'opus', 'Third iteration should stay on opus');
    });
  });
});
