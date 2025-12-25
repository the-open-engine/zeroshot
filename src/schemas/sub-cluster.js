/**
 * Sub-Cluster Schema
 *
 * Defines the structure for nested clusters where an agent can be replaced
 * with an entire cluster configuration, enabling recursive composition.
 *
 * Example:
 * {
 *   "id": "implementation-cluster",
 *   "type": "subcluster",
 *   "config": {
 *     "agents": [
 *       { "id": "worker", "role": "implementation", ... },
 *       { "id": "validator", "role": "validator", ... }
 *     ]
 *   },
 *   "triggers": [{ "topic": "PLAN_READY" }],
 *   "hooks": {
 *     "onComplete": {
 *       "action": "publish_message",
 *       "config": { "topic": "IMPLEMENTATION_COMPLETE" }
 *     }
 *   }
 * }
 */

/**
 * Validate sub-cluster agent configuration
 * @param {Object} agentConfig - Agent config with type: 'subcluster'
 * @param {Number} depth - Current nesting depth (for recursion limit)
 * @returns {{ valid: boolean, errors: string[], warnings: string[] }}
 */
function validateSubCluster(agentConfig, depth = 0) {
  const errors = [];
  const warnings = [];

  // Max nesting depth to prevent infinite recursion
  const MAX_DEPTH = 5;

  if (depth > MAX_DEPTH) {
    errors.push(`Sub-cluster '${agentConfig.id}' exceeds max nesting depth (${MAX_DEPTH})`);
    return { valid: false, errors, warnings };
  }

  // Validate required fields
  if (agentConfig.type !== 'subcluster') {
    errors.push(`Agent '${agentConfig.id}' must have type: 'subcluster'`);
  }

  if (!agentConfig.config) {
    errors.push(`Sub-cluster '${agentConfig.id}' missing config field`);
    return { valid: false, errors, warnings };
  }

  if (!agentConfig.config.agents || !Array.isArray(agentConfig.config.agents)) {
    errors.push(`Sub-cluster '${agentConfig.id}' config.agents must be an array`);
    return { valid: false, errors, warnings };
  }

  if (agentConfig.config.agents.length === 0) {
    errors.push(`Sub-cluster '${agentConfig.id}' config.agents cannot be empty`);
    return { valid: false, errors, warnings };
  }

  // Recursively validate nested cluster config
  const configValidator = require('../config-validator');
  const childValidation = configValidator.validateConfig(agentConfig.config, depth + 1);

  if (!childValidation.valid) {
    errors.push(...childValidation.errors.map((e) => `Sub-cluster '${agentConfig.id}': ${e}`));
  }

  warnings.push(...childValidation.warnings.map((w) => `Sub-cluster '${agentConfig.id}': ${w}`));

  // Validate triggers (sub-cluster must have triggers to activate)
  if (!agentConfig.triggers || agentConfig.triggers.length === 0) {
    errors.push(`Sub-cluster '${agentConfig.id}' must have triggers to activate`);
  }

  // Validate hooks structure
  if (agentConfig.hooks) {
    if (agentConfig.hooks.onComplete) {
      const hook = agentConfig.hooks.onComplete;
      if (!hook.action) {
        errors.push(`Sub-cluster '${agentConfig.id}' onComplete hook missing action`);
      }
      if (hook.action === 'publish_message' && !hook.config?.topic) {
        errors.push(`Sub-cluster '${agentConfig.id}' onComplete hook missing config.topic`);
      }
    }
  }

  // Check for context bridging configuration
  if (agentConfig.contextStrategy?.parentTopics) {
    if (!Array.isArray(agentConfig.contextStrategy.parentTopics)) {
      errors.push(`Sub-cluster '${agentConfig.id}' contextStrategy.parentTopics must be an array`);
    } else {
      // Validate each parent topic is a string
      for (const topic of agentConfig.contextStrategy.parentTopics) {
        if (typeof topic !== 'string') {
          errors.push(
            `Sub-cluster '${agentConfig.id}' parentTopics must contain strings, got ${typeof topic}`
          );
        }
      }
    }
  }

  return {
    valid: errors.length === 0,
    errors,
    warnings,
  };
}

/**
 * Get default sub-cluster template
 * @returns {Object} Default sub-cluster configuration
 */
function getDefaultSubCluster() {
  return {
    id: 'example-subcluster',
    type: 'subcluster',
    role: 'orchestrator',
    config: {
      agents: [
        {
          id: 'worker',
          role: 'implementation',
          triggers: [{ topic: 'PARENT_TRIGGER' }],
          hooks: {
            onComplete: {
              action: 'publish_message',
              config: { topic: 'WORK_COMPLETE' },
            },
          },
        },
      ],
    },
    triggers: [{ topic: 'START_WORK' }],
    hooks: {
      onComplete: {
        action: 'publish_message',
        config: { topic: 'SUBCLUSTER_COMPLETE' },
      },
    },
    contextStrategy: {
      parentTopics: ['ISSUE_OPENED', 'PLAN_READY'],
    },
  };
}

module.exports = {
  validateSubCluster,
  getDefaultSubCluster,
};
