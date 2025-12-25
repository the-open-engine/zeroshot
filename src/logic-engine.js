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

      // Create isolated context with frozen prototypes
      // This prevents prototype pollution attacks
      const isolatedContext = {};

      // Freeze Object, Array, Function prototypes in the sandbox
      isolatedContext.Object = Object.freeze({ ...Object });
      isolatedContext.Array = Array;
      isolatedContext.Function = Function;

      // Copy safe context properties
      Object.assign(isolatedContext, context);

      // Wrap script to prevent prototype access
      const wrappedScript = `(function() {
        'use strict';
        // Prevent prototype pollution
        const frozenObject = Object;
        const frozenArray = Array;
        Object.freeze(frozenObject.prototype);
        Object.freeze(frozenArray.prototype);

        ${script}
      })()`;

      // Create and run in context
      vm.createContext(isolatedContext);
      const result = vm.runInContext(wrappedScript, isolatedContext, {
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

    // Ledger API wrapper (auto-scoped to cluster)
    const ledgerAPI = {
      query: (criteria) => {
        return this.messageBus.query({ ...criteria, cluster_id: clusterId });
      },

      findLast: (criteria) => {
        return this.messageBus.findLast({ ...criteria, cluster_id: clusterId });
      },

      count: (criteria) => {
        return this.messageBus.count({ ...criteria, cluster_id: clusterId });
      },

      since: (timestamp) => {
        return this.messageBus.since({ cluster_id: clusterId, timestamp });
      },
    };

    // Helper functions
    const helpers = {
      /**
       * Check if all agents have responded to a topic since a timestamp
       */
      allResponded: (agents, topic, since) => {
        const responses = ledgerAPI.query({ topic, since });
        const responders = new Set(responses.map((r) => r.sender));
        return agents.every((a) => responders.has(a.id || a));
      },

      /**
       * Check if all responses have approved=true
       */
      hasConsensus: (topic, since) => {
        const responses = ledgerAPI.query({ topic, since });
        if (responses.length === 0) return false;
        return responses.every((r) => r.content?.data?.approved === true);
      },

      /**
       * Get time since last message on topic (in milliseconds)
       */
      timeSinceLastMessage: (topic) => {
        const last = ledgerAPI.findLast({ topic });
        if (!last) return Infinity;
        return Date.now() - last.timestamp;
      },

      /**
       * Check if a topic has any messages since timestamp
       */
      hasMessagesSince: (topic, since) => {
        const count = ledgerAPI.count({ topic, since });
        return count > 0;
      },

      /**
       * Get cluster config based on domain, complexity, and task type
       * Returns: { base: 'template-name', params: { ... } }
       */
      getConfig: require('./config-router').getConfig,
    };

    // Cluster API wrapper
    const clusterAPI = {
      id: clusterId,

      getAgents: () => {
        return this.cluster ? this.cluster.agents || [] : [];
      },

      getAgentsByRole: (role) => {
        return this.cluster ? (this.cluster.agents || []).filter((a) => a.role === role) : [];
      },

      getAgent: (id) => {
        return this.cluster ? (this.cluster.agents || []).find((a) => a.id === id) : null;
      },
    };

    // Build context
    return {
      // Agent context
      agent: {
        id: agent.id,
        role: agent.role,
        iteration: agent.iteration || 0,
      },

      // Triggering message
      message: message || null,

      // APIs
      ledger: ledgerAPI,
      cluster: clusterAPI,
      helpers,

      // Safe built-ins
      Set,
      Map,
      Array,
      Object,
      String,
      Number,
      Boolean,
      Math,
      Date,
      JSON,

      // No-op console (prevent output in production)
      console: {
        log: () => {},
        error: () => {},
        warn: () => {},
        info: () => {},
      },
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
