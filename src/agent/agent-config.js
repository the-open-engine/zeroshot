/**
 * AgentConfig - Agent configuration validation and defaults
 *
 * Provides:
 * - Config validation and normalization
 * - Default values for optional fields
 * - Model configuration setup
 * - Safety checks for test mode
 */

// Default max iterations (high limit - let the user decide when to give up)
const DEFAULT_MAX_ITERATIONS = 100;

// Task timeout - DISABLED (tasks run until completion or explicit kill)
// Originally: 2 hours - caused premature termination of long-running tasks
// Now: Infinity - tasks only stop on completion, explicit kill, or external error
const DEFAULT_TASK_TIMEOUT_MS = Infinity;

// Stale detection - ENABLED by default using multi-indicator analysis (safe from false positives)
// Multi-indicator approach checks: process state, CPU usage, context switches, network I/O
// Only flags as stuck when ALL indicators show inactivity (score >= 3.5)
// Single-indicator detection (just output freshness) was too risky - this is safe.
const DEFAULT_STALE_DURATION_MS = 30 * 60 * 1000; // 30 minutes before triggering analysis
const DEFAULT_LIVENESS_CHECK_ENABLED = true; // Safe with multi-indicator detection

/**
 * Validate and normalize agent configuration
 * @param {Object} config - Raw agent configuration
 * @param {Object} options - Agent wrapper options
 * @returns {Object} Normalized configuration
 */
function validateAgentConfig(config, options = {}) {
  // CRITICAL: Enforce JSON schema output by default to prevent parse failures and crashes
  // Agents MUST return structured output so hooks can safely use {{result.*}} templates
  if (!config.outputFormat) {
    config.outputFormat = 'json';
  }

  // If outputFormat is json but no schema defined, use a minimal default schema
  // This prevents uncaught exceptions when parsing agent output
  if (config.outputFormat === 'json' && !config.jsonSchema) {
    config.jsonSchema = {
      type: 'object',
      properties: {
        summary: {
          type: 'string',
          description: 'Brief summary of what was done',
        },
        result: {
          type: 'string',
          description: 'Detailed result or output',
        },
      },
      required: ['summary', 'result'],
    };
  }

  // Model configuration: support both static model and dynamic rules
  let modelConfig;
  if (config.modelRules) {
    modelConfig = { type: 'rules', rules: config.modelRules };
  } else {
    modelConfig = { type: 'static', model: config.model || 'sonnet' };
  }

  // Prompt configuration: support static prompt OR iteration-based rules
  // Formats:
  //   prompt: "string"                              -> static
  //   prompt: { system: "string" }                  -> static
  //   prompt: { initial: "...", subsequent: "..." } -> iteration 1 vs 2+
  //   prompt: { iterations: [...] }                 -> full control
  let promptConfig = null;
  if (config.prompt?.iterations) {
    promptConfig = { type: 'rules', rules: config.prompt.iterations };
  } else if (config.prompt?.initial || config.prompt?.subsequent) {
    const rules = [];
    if (config.prompt.initial) rules.push({ match: '1', system: config.prompt.initial });
    if (config.prompt.subsequent) rules.push({ match: '2+', system: config.prompt.subsequent });
    promptConfig = { type: 'rules', rules };
  } else if (typeof config.prompt === 'string') {
    promptConfig = { type: 'static', system: config.prompt };
  } else if (config.prompt?.system) {
    promptConfig = { type: 'static', system: config.prompt.system };
  } else if (config.prompt) {
    throw new Error(`Agent "${config.id}": invalid prompt format`);
  }

  // Build normalized config
  const normalizedConfig = {
    ...config,
    modelConfig,
    promptConfig,
    maxIterations: config.maxIterations || DEFAULT_MAX_ITERATIONS,
    timeout: config.timeout || DEFAULT_TASK_TIMEOUT_MS,
    staleDuration: config.staleDuration || DEFAULT_STALE_DURATION_MS,
    enableLivenessCheck: config.enableLivenessCheck ?? DEFAULT_LIVENESS_CHECK_ENABLED, // On by default, opt-out with false
  };

  // SAFETY: In test mode, verify mock is provided for agents that execute tasks
  // Check if this agent executes tasks (vs orchestrator agents that only publish messages)
  const executesTask = config.triggers?.some(
    (trigger) => !trigger.action || trigger.action === 'execute_task'
  );

  if (options.testMode && !options.mockSpawnFn && executesTask) {
    throw new Error(
      `AgentWrapper: testMode=true but no mockSpawnFn provided for agent '${config.id}'. ` +
        `This would cause real Claude API calls. ABORTING.`
    );
  }

  return normalizedConfig;
}

module.exports = {
  validateAgentConfig,
  DEFAULT_MAX_ITERATIONS,
  DEFAULT_TASK_TIMEOUT_MS,
  DEFAULT_STALE_DURATION_MS,
  DEFAULT_LIVENESS_CHECK_ENABLED,
};
