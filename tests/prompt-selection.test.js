const assert = require('assert');
const AgentWrapper = require('../src/agent-wrapper');

// Mock message bus and cluster
const mockMessageBus = { publish: () => {}, subscribe: () => () => {} };
const mockCluster = { id: 'test-cluster', agents: [] };

describe('Prompt Selection - Static (backward compat)', function () {
  it('should use string prompt directly', function () {
    const agent = new AgentWrapper(
      { id: 'test', timeout: 0, prompt: 'You are a helpful assistant.' },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    assert.strictEqual(agent._selectPrompt(), 'You are a helpful assistant.');
  });

  it('should use object prompt with system property', function () {
    const agent = new AgentWrapper(
      { id: 'test', timeout: 0, prompt: { system: 'You are a code reviewer.' } },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    assert.strictEqual(agent._selectPrompt(), 'You are a code reviewer.');
  });

  it('should return null if no prompt configured', function () {
    const agent = new AgentWrapper(
      { id: 'test', timeout: 0 },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    assert.strictEqual(agent._selectPrompt(), null);
  });
});

describe('Prompt Selection - initial/subsequent shorthand', function () {
  it('should use initial prompt on iteration 1', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: {
          initial: 'Start fresh. Analyze the task.',
          subsequent: 'Address the feedback.',
        },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 1;
    assert.strictEqual(agent._selectPrompt(), 'Start fresh. Analyze the task.');
  });

  it('should use subsequent prompt on iteration 2+', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: {
          initial: 'Start fresh. Analyze the task.',
          subsequent: 'Address the feedback.',
        },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 2;
    assert.strictEqual(agent._selectPrompt(), 'Address the feedback.');

    agent.iteration = 5;
    assert.strictEqual(agent._selectPrompt(), 'Address the feedback.');

    agent.iteration = 100;
    assert.strictEqual(agent._selectPrompt(), 'Address the feedback.');
  });

  it('should work with only initial prompt', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: { initial: 'First iteration only.' },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 1;
    assert.strictEqual(agent._selectPrompt(), 'First iteration only.');
  });

  it('should work with only subsequent prompt', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: { subsequent: 'Retry iterations.' },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 2;
    assert.strictEqual(agent._selectPrompt(), 'Retry iterations.');

    agent.iteration = 10;
    assert.strictEqual(agent._selectPrompt(), 'Retry iterations.');
  });
});

describe('Prompt Selection - Full iterations array', function () {
  it('should match exact iteration', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: {
          iterations: [
            { match: '1', system: 'First iteration prompt.' },
            { match: '2', system: 'Second iteration prompt.' },
            { match: '3+', system: 'Third+ iteration prompt.' },
          ],
        },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 1;
    assert.strictEqual(agent._selectPrompt(), 'First iteration prompt.');

    agent.iteration = 2;
    assert.strictEqual(agent._selectPrompt(), 'Second iteration prompt.');
  });

  it('should match range', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: {
          iterations: [
            { match: '1-3', system: 'Early iterations.' },
            { match: 'all', system: 'Fallback prompt.' },
          ],
        },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 1;
    assert.strictEqual(agent._selectPrompt(), 'Early iterations.');

    agent.iteration = 3;
    assert.strictEqual(agent._selectPrompt(), 'Early iterations.');

    agent.iteration = 4;
    assert.strictEqual(agent._selectPrompt(), 'Fallback prompt.');
  });

  it('should match open-ended range', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: {
          iterations: [
            { match: '1', system: 'First only.' },
            { match: '2+', system: 'Second and beyond.' },
          ],
        },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 1;
    assert.strictEqual(agent._selectPrompt(), 'First only.');

    agent.iteration = 2;
    assert.strictEqual(agent._selectPrompt(), 'Second and beyond.');

    agent.iteration = 100;
    assert.strictEqual(agent._selectPrompt(), 'Second and beyond.');
  });

  it('should use catch-all as fallback', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: {
          iterations: [
            { match: '1', system: 'First iteration.' },
            { match: 'all', system: 'All other iterations.' },
          ],
        },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 999;
    assert.strictEqual(agent._selectPrompt(), 'All other iterations.');
  });

  it('should use first matching rule', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: {
          iterations: [
            { match: '1-10', system: 'Early prompt.' },
            { match: '5+', system: 'Should not match.' },
          ],
        },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 7;
    assert.strictEqual(agent._selectPrompt(), 'Early prompt.'); // First rule wins
  });
});

describe('Prompt Selection - Error handling', function () {
  it('should throw if no rules match', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: {
          iterations: [{ match: '1-2', system: 'Limited range.' }],
        },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 5;
    assert.throws(() => agent._selectPrompt(), /No prompt rule matched iteration 5/);
  });

  it('should throw on invalid prompt format', function () {
    assert.throws(
      () =>
        new AgentWrapper(
          { id: 'test', timeout: 0, prompt: { invalid: 'format' } },
          mockMessageBus,
          mockCluster,
          { testMode: true, mockSpawnFn: () => {} }
        ),
      /invalid prompt format/
    );
  });

  it('should throw if only subsequent but iteration is 1', function () {
    const agent = new AgentWrapper(
      {
        id: 'test',
        timeout: 0,
        prompt: { subsequent: 'Only for 2+.' },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 1;
    assert.throws(() => agent._selectPrompt(), /No prompt rule matched iteration 1/);
  });
});

describe('Prompt Selection - Real-world use case', function () {
  it('should use different prompts for first vs retry iterations', function () {
    const initialPrompt = `You are implementing a feature from scratch.
Read the issue carefully and implement it fully.`;
    const subsequentPrompt = `You are iterating based on validator feedback.
Focus only on fixing what was rejected.`;

    const agent = new AgentWrapper(
      {
        id: 'worker',
        timeout: 0,
        prompt: { initial: initialPrompt, subsequent: subsequentPrompt },
      },
      mockMessageBus,
      mockCluster,
      { testMode: true, mockSpawnFn: () => {} }
    );

    agent.iteration = 1;
    assert.ok(
      agent._selectPrompt().includes('implementing a feature from scratch'),
      'First iteration should get initial prompt'
    );

    agent.iteration = 2;
    assert.ok(
      agent._selectPrompt().includes('iterating based on validator feedback'),
      'Second iteration should get subsequent prompt'
    );

    agent.iteration = 3;
    assert.ok(
      agent._selectPrompt().includes('iterating based on validator feedback'),
      'Third iteration should still get subsequent prompt'
    );
  });
});
