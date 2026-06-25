/**
 * LogicEngine - JavaScript sandbox for agent decision logic
 *
 * Provides:
 * - Isolated VM for executing agent trigger logic
 * - Timeout enforcement (1 second)
 * - Ledger API access for queries
 * - Helper functions for common patterns
 * - Sandbox security (no fs, network, child_process)
 */

const vm = require('vm');

function buildLedgerAPI(messageBus, clusterId) {
  return {
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
}

function buildHelpers(ledgerAPI) {
  return {
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

    timeSinceLastMessage: (topic) => {
      const last = ledgerAPI.findLast({ topic });
      if (!last) return Infinity;
      return Date.now() - last.timestamp;
    },

    hasMessagesSince: (topic, since) => {
      const count = ledgerAPI.count({ topic, since });
      return count > 0;
    },

    getConfig: require('./config-router').getConfig,
  };
}

function buildClusterAPI(cluster, clusterId) {
  const getAgents = () => (cluster ? cluster.agents || [] : []);

  return {
    id: clusterId,
    getAgents,
    getAgentsByRole: (role) => getAgents().filter((a) => a.role === role),
    getAgent: (id) => getAgents().find((a) => a.id === id) || null,
  };
}

function buildAgentContext(agent) {
  return {
    id: agent.id,
    role: agent.role,
    iteration: agent.iteration || 0,
    requiredQualityGates: Array.isArray(agent.requiredQualityGates)
      ? agent.requiredQualityGates
      : [],
  };
}

function getQuietConsole() {
  return {
    log: () => {},
    error: () => {},
    warn: () => {},
    info: () => {},
  };
}

class LogicEngine {
  constructor(messageBus, cluster) {
    this.messageBus = messageBus;
    this.cluster = cluster;
    this.timeout = 1000; // 1 second
  }

  /**
   * Evaluate a trigger logic script
   * @param {String} script - JavaScript code to evaluate
   * @param {Object} agent - Agent context
   * @param {Object} message - Triggering message
   * @returns {Boolean} Whether agent should wake up
   */
  evaluate(script, agent, message) {
    try {
      // Build sandbox context
      const context = this._buildContext(agent, message);

      // Contextify before execution so scripts use VM-owned globals instead of
      // host constructors. Keep the function-body contract: trigger scripts use
      // return statements throughout built-in templates and user configs.
      const sandbox = { ...context };
      vm.createContext(sandbox);

      const wrappedScript = `(function() {
        'use strict';
        ${script}
      })()`;

      // Run in context
      const result = vm.runInContext(wrappedScript, sandbox, {
        timeout: this.timeout,
        displayErrors: true,
      });

      // Coerce to boolean
      return Boolean(result);
    } catch (error) {
      console.error(`Logic evaluation error for agent ${agent.id}:`, error.message);
      return false; // Default to false on error
    }
  }

  /**
   * Build sandbox context with APIs and helpers
   * @private
   */
  _buildContext(agent, message) {
    const clusterId = agent.cluster_id;
    const ledgerAPI = buildLedgerAPI(this.messageBus, clusterId);

    // Build context
    return {
      agent: buildAgentContext(agent),
      message: message || null,
      ledger: ledgerAPI,
      cluster: buildClusterAPI(this.cluster, clusterId),
      helpers: buildHelpers(ledgerAPI),
      console: getQuietConsole(),
    };
  }

  /**
   * Validate script syntax without executing
   * @param {String} script - JavaScript code
   * @returns {Object} { valid: Boolean, error: String }
   */
  validateScript(script) {
    try {
      // Wrap in function like evaluate() does
      const wrappedScript = `(function() { ${script} })()`;
      new vm.Script(wrappedScript);
      return { valid: true, error: null };
    } catch (error) {
      return { valid: false, error: error.message };
    }
  }

  /**
   * Set timeout for script execution
   * @param {Number} ms - Timeout in milliseconds
   */
  setTimeout(ms) {
    this.timeout = ms;
  }
}

module.exports = LogicEngine;
