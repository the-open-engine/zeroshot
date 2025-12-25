/**
 * AgentHookExecutor - Hook transformation and execution
 *
 * Provides:
 * - Hook execution (publish_message, stop_cluster, etc.)
 * - Template variable substitution
 * - Transform script execution in VM sandbox
 */

const vm = require('vm');

/**
 * Execute a hook
 * THROWS on failure - no silent errors
 * @param {Object} params - Hook execution parameters
 * @param {Object} params.hook - Hook configuration
 * @param {Object} params.agent - Agent instance
 * @param {Object} params.message - Triggering message
 * @param {Object} params.result - Agent execution result
 * @param {Object} params.messageBus - Message bus instance
 * @param {Object} params.cluster - Cluster object
 * @param {Object} params.orchestrator - Orchestrator instance
 * @returns {Promise<void>}
 */
function executeHook(params) {
  const { hook, agent, message, result, cluster } = params;

  if (!hook) {
    return;
  }

  // Build context for hook execution
  const context = {
    result,
    triggeringMessage: message,
    agent,
    cluster,
  };

  // NO try/catch - errors must propagate
  if (hook.action === 'publish_message') {
    let messageToPublish;

    if (hook.transform) {
      // NEW: Execute transform script to generate message
      messageToPublish = executeTransform({
        transform: hook.transform,
        context,
        agent,
      });
    } else {
      // Existing: Use template substitution
      messageToPublish = substituteTemplate({
        config: hook.config,
        context,
        agent,
        cluster,
      });
    }

    // Publish via agent's _publish method
    agent._publish(messageToPublish);
  } else if (hook.action === 'execute_system_command') {
    throw new Error('execute_system_command not implemented');
  } else {
    throw new Error(`Unknown hook action: ${hook.action}`);
  }
}

/**
 * Execute a hook transform script
 * Transform scripts return the message to publish, with access to:
 * - result: parsed agent output
 * - triggeringMessage: the message that triggered the agent
 * - helpers: { getConfig(domain, complexity, taskType) }
 * @param {Object} params - Transform parameters
 * @param {Object} params.transform - Transform configuration
 * @param {Object} params.context - Execution context
 * @param {Object} params.agent - Agent instance
 * @returns {Object} Message to publish
 */
function executeTransform(params) {
  const { transform, context, agent } = params;
  const { engine, script } = transform;

  if (engine !== 'javascript') {
    throw new Error(`Unsupported transform engine: ${engine}`);
  }

  // Parse result output if we have a result
  // VALIDATION: Check if script uses result.* variables and fail early if no output
  const scriptUsesResult = /\bresult\.[a-zA-Z]/.test(script);
  let resultData = null;

  if (context.result?.output) {
    resultData = agent._parseResultOutput(context.result.output);
  } else if (scriptUsesResult) {
    const taskId = context.result?.taskId || agent.currentTaskId || 'UNKNOWN';
    const outputLength = (context.result?.output || '').length;
    throw new Error(
      `Transform script uses result.* variables but no output was captured. ` +
        `Agent: ${agent.id}, TaskID: ${taskId}, Iteration: ${agent.iteration}, ` +
        `Output length: ${outputLength}. ` +
        `Check that the task completed successfully and the get-log-path command exists.`
    );
  }

  // Helper functions exposed to transform scripts
  const helpers = {
    /**
     * Get cluster config based on domain, complexity, and task type
     * Returns: { base: 'template-name', params: { ... } }
     */
    getConfig: require('../config-router').getConfig,
  };

  // Build sandbox context
  const sandbox = {
    result: resultData,
    triggeringMessage: context.triggeringMessage,
    helpers,
    JSON,
    console: {
      log: (...args) => agent._log('[transform]', ...args),
      error: (...args) => console.error('[transform]', ...args),
      warn: (...args) => console.warn('[transform]', ...args),
    },
  };

  // Execute in VM sandbox with timeout
  const vmContext = vm.createContext(sandbox);
  const wrappedScript = `(function() { ${script} })()`;

  let result;
  try {
    result = vm.runInContext(wrappedScript, vmContext, { timeout: 5000 });
  } catch (err) {
    throw new Error(`Transform script error: ${err.message}`);
  }

  // Validate result
  if (!result || typeof result !== 'object') {
    throw new Error(
      `Transform script must return an object with topic and content, got: ${typeof result}`
    );
  }
  if (!result.topic) {
    throw new Error(`Transform script result must have a 'topic' property`);
  }
  if (!result.content) {
    throw new Error(`Transform script result must have a 'content' property`);
  }

  return result;
}

/**
 * Substitute template variables in hook config
 * ONLY parses result output if result.* variables are used
 * THROWS on any error - no silent failures
 * @param {Object} params - Substitution parameters
 * @param {Object} params.config - Hook configuration
 * @param {Object} params.context - Execution context
 * @param {Object} params.agent - Agent instance
 * @param {Object} params.cluster - Cluster object
 * @returns {Object} Substituted configuration
 */
function substituteTemplate(params) {
  const { config, context, agent, cluster } = params;

  if (!config) {
    throw new Error('_substituteTemplate: config is required');
  }

  const json = JSON.stringify(config);

  // Check if ANY result.* variables are used BEFORE parsing
  // Generic pattern - no hardcoded field names, works with any agent config
  const usesResultVars = /\{\{result\.[^}]+\}\}/.test(json);

  let resultData = null;
  if (usesResultVars) {
    if (!context.result) {
      throw new Error(
        `Hook uses result.* variables but no result in context. ` +
          `Agent: ${agent.id}, TaskID: ${agent.currentTaskId}, Iteration: ${agent.iteration}`
      );
    }
    if (!context.result.output) {
      // Log detailed context for debugging
      const taskId = context.result.taskId || agent.currentTaskId || 'UNKNOWN';
      console.error(`\n${'='.repeat(80)}`);
      console.error(`🔴 HOOK FAILURE - EMPTY OUTPUT`);
      console.error(`${'='.repeat(80)}`);
      console.error(`Agent: ${agent.id}`);
      console.error(`Task ID: ${taskId}`);
      console.error(`Iteration: ${context.result.iteration || agent.iteration}`);
      console.error(`Result success: ${context.result.success}`);
      console.error(`Result error: ${context.result.error || 'none'}`);
      console.error(`Output length: ${(context.result.output || '').length}`);
      console.error(`Hook config: ${JSON.stringify(config, null, 2)}`);

      // Auto-fetch and publish task logs for debugging
      let taskLogs = 'Task logs unavailable';
      if (taskId !== 'UNKNOWN') {
        console.error(`\nFetching task logs for ${taskId}...`);
        try {
          const { execSync } = require('child_process');
          const ctPath = agent._getClaudeTasksPath();
          taskLogs = execSync(`${ctPath} logs ${taskId} --lines 100`, {
            encoding: 'utf-8',
            timeout: 5000,
            maxBuffer: 1024 * 1024, // 1MB
          }).trim();
          console.error(`✓ Retrieved ${taskLogs.split('\n').length} lines of logs`);
        } catch (err) {
          taskLogs = `Failed to retrieve logs: ${err.message}`;
          console.error(`✗ Failed to retrieve logs: ${err.message}`);
        }
      }
      console.error(`${'='.repeat(80)}\n`);

      // Publish task logs to message bus for visibility in zeroshot logs
      agent._publish({
        topic: 'AGENT_ERROR',
        receiver: 'broadcast',
        content: {
          text: `Task logs for ${taskId} (last 100 lines)`,
          data: {
            taskId,
            logs: taskLogs,
            logsPreview: taskLogs.split('\n').slice(-20).join('\n'), // Last 20 lines as preview
          },
        },
      });

      throw new Error(
        `Hook uses result.* variables but result.output is empty. ` +
          `Agent: ${agent.id}, TaskID: ${taskId}, ` +
          `Iteration: ${context.result.iteration || agent.iteration}, ` +
          `Success: ${context.result.success}. ` +
          `Task logs posted to message bus.`
      );
    }
    // Parse result output - WILL THROW if no JSON block
    resultData = agent._parseResultOutput(context.result.output);
  }

  // Helper to escape a value for JSON string substitution
  // Uses JSON.stringify for ALL escaping - no manual replace() calls
  const escapeForJsonString = (value) => {
    if (value === null || value === undefined) {
      throw new Error(`Cannot escape null/undefined value for JSON`);
    }
    // JSON.stringify handles ALL escaping (newlines, quotes, backslashes, control chars)
    // .slice(1, -1) strips the outer quotes it adds
    // For arrays/objects: stringify twice - once for JSON, once to escape for string embedding
    const stringified =
      typeof value === 'string' ? JSON.stringify(value) : JSON.stringify(JSON.stringify(value));
    return stringified.slice(1, -1);
  };

  let substituted = json
    .replace(/\{\{cluster\.id\}\}/g, cluster.id)
    .replace(/\{\{cluster\.createdAt\}\}/g, String(cluster.createdAt))
    .replace(/\{\{iteration\}\}/g, String(agent.iteration))
    .replace(/\{\{error\.message\}\}/g, escapeForJsonString(context.error?.message ?? ''))
    .replace(/\{\{result\.output\}\}/g, escapeForJsonString(context.result?.output ?? ''));

  // Substitute ALL result.* variables dynamically from parsed resultData
  if (resultData) {
    // Generic substitution - replace {{result.fieldName}} with resultData[fieldName]
    // No hardcoded field names - works with any agent output schema
    // CRITICAL: For booleans/nulls/numbers, we need to match and remove surrounding quotes
    // to produce valid JSON (e.g., "{{result.approved}}" -> true, not "true")
    substituted = substituted.replace(/"?\{\{result\.([^}]+)\}\}"?/g, (match, fieldName) => {
      const value = resultData[fieldName];
      if (value === undefined) {
        // Missing fields should gracefully default to null or empty values
        // This allows optional schema fields without hardcoding field names
        // If a field is truly required, the schema validation will catch it
        console.warn(
          `⚠️  Agent ${agent.id}: Template variable {{result.${fieldName}}} not found in output. ` +
            `If this field is required by the schema, the agent violated its own schema. ` +
            `Defaulting to null. Agent output keys: ${Object.keys(resultData).join(', ')}`
        );
        return 'null';
      }
      // Booleans, numbers, and null should be unquoted JSON primitives
      if (typeof value === 'boolean' || typeof value === 'number' || value === null) {
        return String(value);
      }
      // Strings need to be quoted and escaped for JSON
      return JSON.stringify(value);
    });
  }

  // Check for unsubstituted KNOWN template variables only
  // KNOWN patterns: {{cluster.X}}, {{iteration}}, {{error.X}}, {{result.X}}
  // Content may contain arbitrary {{...}} patterns (React dangerouslySetInnerHTML, Mustache, etc.)
  // Those are NOT template variables - they're just content that happens to contain braces
  const KNOWN_TEMPLATE_PREFIXES = ['cluster', 'iteration', 'error', 'result'];
  const knownVariablePattern = new RegExp(
    `\\{\\{(${KNOWN_TEMPLATE_PREFIXES.join('|')})(\\.[a-zA-Z_][a-zA-Z0-9_]*)?\\}\\}`,
    'g'
  );
  const remaining = substituted.match(knownVariablePattern);
  if (remaining) {
    throw new Error(`Unsubstituted template variables: ${remaining.join(', ')}`);
  }

  // Parse and validate result
  let result;
  try {
    result = JSON.parse(substituted);
  } catch (e) {
    console.error('JSON parse failed. Substituted string:');
    console.error(substituted);
    throw new Error(`Template substitution produced invalid JSON: ${e.message}`);
  }

  return result;
}

module.exports = {
  executeHook,
  executeTransform,
  substituteTemplate,
};
