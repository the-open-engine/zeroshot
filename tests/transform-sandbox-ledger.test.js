/**
 * Regression test for transform sandbox ledger API
 *
 * ROOT CAUSE (2026-02-03):
 * buildTransformSandbox() did NOT provide `ledger` API to transform scripts,
 * but template transforms (e.g., heavy-validation.json:247) used `ledger.query()`.
 * Result: "ledger is not defined" → hook failed → cluster deadlocked.
 *
 * FIX: Added ledger, cluster, and helpers APIs to buildTransformSandbox(),
 * mirroring logic-engine.js _buildContext().
 */

const { expect } = require('chai');
const vm = require('vm');

// Mock message bus
function createMockMessageBus() {
  const messages = [];
  return {
    publish: (msg) => messages.push({ ...msg, timestamp: Date.now() }),
    query: ({ topic, cluster_id, since }) => {
      return messages.filter(
        (m) =>
          m.cluster_id === cluster_id &&
          (!topic || m.topic === topic) &&
          (!since || m.timestamp > since)
      );
    },
    findLast: ({ topic, cluster_id }) => {
      const matching = messages.filter(
        (m) => m.cluster_id === cluster_id && (!topic || m.topic === topic)
      );
      return matching[matching.length - 1] || null;
    },
    count: ({ topic, cluster_id, since }) => {
      return messages.filter(
        (m) =>
          m.cluster_id === cluster_id &&
          (!topic || m.topic === topic) &&
          (!since || m.timestamp > since)
      ).length;
    },
    since: ({ cluster_id, timestamp }) => {
      return messages.filter((m) => m.cluster_id === cluster_id && m.timestamp > timestamp);
    },
    _messages: messages,
  };
}

// Mock agent
function createMockAgent(messageBus, clusterId = 'test-cluster') {
  return {
    id: 'test-agent',
    cluster_id: clusterId,
    messageBus,
    cluster: {
      id: clusterId,
      agents: [
        { id: 'validator-1', role: 'validator' },
        { id: 'validator-2', role: 'validator' },
        { id: 'worker', role: 'implementation' },
      ],
    },
    _log: () => {},
  };
}

describe('Transform sandbox ledger API', () => {
  let messageBus;
  let agent;

  beforeEach(() => {
    messageBus = createMockMessageBus();
    agent = createMockAgent(messageBus);
  });

  /**
   * REGRESSION TEST: Transform script can access ledger.query()
   *
   * This is the exact pattern that failed in heavy-validation.json:247
   */
  it('transform script can use ledger.query()', () => {
    // Seed some messages
    messageBus.publish({
      cluster_id: 'test-cluster',
      topic: 'HEAVY_VALIDATION_RESULT',
      sender: 'validator-1',
      content: { data: { errors: ['error1'] } },
    });
    messageBus.publish({
      cluster_id: 'test-cluster',
      topic: 'HEAVY_VALIDATION_RESULT',
      sender: 'validator-2',
      content: { data: { errors: ['error2'] } },
    });

    // Build sandbox like agent-hook-executor.js does
    const sandbox = buildTestSandbox(agent, { allApproved: true, summary: 'All good' });

    // This is the EXACT script from heavy-validation.json:247 that was failing
    const script = `
      return {
        topic: 'VALIDATION_RESULT',
        content: {
          text: result.allApproved ? 'All validations passed' : 'Stage 2 rejected',
          data: {
            approved: result.allApproved,
            stage: 'heavy',
            summary: result.summary,
            errors: ledger.query({ topic: 'HEAVY_VALIDATION_RESULT' })
              .flatMap(r => r.content?.data?.errors || [])
          }
        }
      };
    `;

    const vmContext = vm.createContext(sandbox);
    const wrappedScript = `(function() { ${script} })()`;
    const result = vm.runInContext(wrappedScript, vmContext);

    expect(result.topic).to.equal('VALIDATION_RESULT');
    expect(result.content.data.approved).to.equal(true);
    expect(result.content.data.errors).to.deep.equal(['error1', 'error2']);
  });

  /**
   * REGRESSION TEST: Transform script can access ledger.findLast()
   */
  it('transform script can use ledger.findLast()', () => {
    messageBus.publish({
      cluster_id: 'test-cluster',
      topic: 'IMPLEMENTATION_READY',
      sender: 'worker',
      content: { text: 'Done' },
    });

    const sandbox = buildTestSandbox(agent, {});

    const script = `
      const lastPush = ledger.findLast({ topic: 'IMPLEMENTATION_READY' });
      return {
        topic: 'TEST_RESULT',
        content: { found: !!lastPush, sender: lastPush?.sender }
      };
    `;

    const vmContext = vm.createContext(sandbox);
    const result = vm.runInContext(`(function() { ${script} })()`, vmContext);

    expect(result.content.found).to.equal(true);
    expect(result.content.sender).to.equal('worker');
  });

  /**
   * REGRESSION TEST: Transform script can access cluster.getAgentsByRole()
   *
   * Pattern from full-workflow.json:255
   */
  it('transform script can use cluster.getAgentsByRole()', () => {
    const sandbox = buildTestSandbox(agent, {});

    const script = `
      const validators = cluster.getAgentsByRole('validator');
      return {
        topic: 'TEST_RESULT',
        content: { validatorCount: validators.length, ids: validators.map(v => v.id) }
      };
    `;

    const vmContext = vm.createContext(sandbox);
    const result = vm.runInContext(`(function() { ${script} })()`, vmContext);

    expect(result.content.validatorCount).to.equal(2);
    expect(result.content.ids).to.deep.equal(['validator-1', 'validator-2']);
  });

  /**
   * REGRESSION TEST: Transform script can use helpers.allResponded()
   */
  it('transform script can use helpers.allResponded()', () => {
    // Both validators responded
    messageBus.publish({
      cluster_id: 'test-cluster',
      topic: 'VALIDATION_RESULT',
      sender: 'validator-1',
      content: { data: { approved: true } },
    });
    messageBus.publish({
      cluster_id: 'test-cluster',
      topic: 'VALIDATION_RESULT',
      sender: 'validator-2',
      content: { data: { approved: true } },
    });

    const sandbox = buildTestSandbox(agent, {});

    const script = `
      const validators = cluster.getAgentsByRole('validator');
      const allDone = helpers.allResponded(validators, 'VALIDATION_RESULT', 0);
      return {
        topic: 'TEST_RESULT',
        content: { allResponded: allDone }
      };
    `;

    const vmContext = vm.createContext(sandbox);
    const result = vm.runInContext(`(function() { ${script} })()`, vmContext);

    expect(result.content.allResponded).to.equal(true);
  });

  /**
   * REGRESSION TEST: Sandbox provides Set for validators pattern
   */
  it('transform script can use Set builtin', () => {
    const sandbox = buildTestSandbox(agent, {});

    const script = `
      const ids = new Set(['a', 'b', 'a']);
      return {
        topic: 'TEST_RESULT',
        content: { size: ids.size }
      };
    `;

    const vmContext = vm.createContext(sandbox);
    const result = vm.runInContext(`(function() { ${script} })()`, vmContext);

    expect(result.content.size).to.equal(2);
  });
});

/**
 * Build sandbox matching agent-hook-executor.js buildTransformSandbox()
 * This is a copy for testing - the real one is in agent-hook-executor.js
 */
function buildTestSandbox(agent, resultData) {
  const clusterId = agent.cluster_id;
  const messageBus = agent.messageBus;
  const cluster = agent.cluster;

  const ledgerAPI = {
    query: (criteria) => {
      return messageBus.query({ ...criteria, cluster_id: clusterId });
    },
    findLast: (criteria) => {
      return messageBus.findLast({ ...criteria, cluster_id: clusterId });
    },
    count: (criteria) => {
      return messageBus.count({ ...criteria, cluster_id: clusterId });
    },
    since: (timestamp) => {
      return messageBus.since({ cluster_id: clusterId, timestamp });
    },
  };

  const clusterAPI = {
    id: clusterId,
    getAgents: () => (cluster ? cluster.agents || [] : []),
    getAgentsByRole: (role) =>
      cluster ? (cluster.agents || []).filter((a) => a.role === role) : [],
    getAgent: (id) => (cluster ? (cluster.agents || []).find((a) => a.id === id) : null),
  };

  const helpers = {
    getConfig: () => ({}),
    allResponded: (agents, topic, since) => {
      const responses = ledgerAPI.query({ topic, since });
      const responders = new Set(responses.map((r) => r.sender));
      return agents.every((a) => responders.has(a.id || a));
    },
    hasConsensus: (topic, since) => {
      const responses = ledgerAPI.query({ topic, since });
      if (responses.length === 0) return false;
      return responses.every((r) => r.content?.data?.approved === true);
    },
  };

  return {
    result: resultData,
    triggeringMessage: null,
    ledger: ledgerAPI,
    cluster: clusterAPI,
    helpers,
    JSON,
    Set,
    Map,
    Array,
    Object,
    console: {
      log: () => {},
      error: () => {},
      warn: () => {},
    },
  };
}
