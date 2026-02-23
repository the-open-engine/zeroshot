/**
 * Hook Template - Template variable substitution for hook configs
 */

const { execSync } = require('../lib/safe-exec');

/**
 * Escape a value for JSON string substitution.
 * Uses JSON.stringify for ALL escaping — no manual replace() calls.
 */
function escapeForJsonString(value) {
  if (value === null || value === undefined) {
    throw new Error(`Cannot escape null/undefined value for JSON`);
  }
  const stringified =
    typeof value === 'string' ? JSON.stringify(value) : JSON.stringify(JSON.stringify(value));
  return stringified.slice(1, -1);
}

/**
 * Escape template-like patterns in substituted values.
 * Prevents content containing "{{result.foo}}" from being flagged as unsubstituted.
 * Uses Unicode escape: {{ -> \u007B{ (invisible to humans, breaks pattern match)
 */
function escapeTemplatePatterns(str) {
  return str.replace(/\{\{/g, '\\u007B{');
}

function substituteStaticVars(json, context, agent, cluster) {
  return json
    .replace(/\{\{cluster\.id\}\}/g, escapeTemplatePatterns(cluster.id))
    .replace(/\{\{cluster\.createdAt\}\}/g, String(cluster.createdAt))
    .replace(/\{\{iteration\}\}/g, String(agent.iteration))
    .replace(
      /\{\{error\.message\}\}/g,
      escapeTemplatePatterns(escapeForJsonString(context.error?.message ?? ''))
    )
    .replace(
      /\{\{result\.output\}\}/g,
      escapeTemplatePatterns(escapeForJsonString(context.result?.output ?? ''))
    );
}

function substituteResultField(match, fieldName, resultData, agent) {
  const value = resultData[fieldName];
  if (value === undefined) {
    console.warn(
      `⚠️  Agent ${agent.id}: Template variable {{result.${fieldName}}} not found in output. ` +
        `If this field is required by the schema, the agent violated its own schema. ` +
        `Defaulting to null. Agent output keys: ${Object.keys(resultData).join(', ')}`
    );
    return 'null';
  }
  if (typeof value === 'boolean' || typeof value === 'number' || value === null) {
    return String(value);
  }
  return escapeTemplatePatterns(JSON.stringify(value));
}

function substituteResultVars(json, resultData, agent) {
  return json.replace(/"?\{\{result\.([^}]+)\}\}"?/g, (match, fieldName) => {
    return substituteResultField(match, fieldName, resultData, agent);
  });
}

function findUnsubstitutedVars(substituted) {
  const prefixes = ['cluster', 'iteration', 'error', 'result'];
  const found = [];
  for (const prefix of prefixes) {
    const marker = `{{${prefix}`;
    let idx = substituted.indexOf(marker);
    while (idx !== -1) {
      const end = substituted.indexOf('}}', idx);
      if (end !== -1) found.push(substituted.slice(idx, end + 2));
      idx = substituted.indexOf(marker, idx + 1);
    }
  }
  return found;
}

function checkUnsubstitutedVars(substituted) {
  const remaining = findUnsubstitutedVars(substituted);
  if (remaining.length > 0) {
    throw new Error(`Unsubstituted template variables: ${remaining.join(', ')}`);
  }
}

function logEmptyOutputError({ agent, context, config }) {
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
  return taskId;
}

function fetchAndPublishTaskLogs({ agent, taskId }) {
  let taskLogs = 'Task logs unavailable';
  if (taskId !== 'UNKNOWN') {
    console.error(`\nFetching task logs for ${taskId}...`);
    try {
      const ctPath = agent._getClaudeTasksPath();
      taskLogs = execSync(`${ctPath} logs ${taskId} --lines 100`, {
        encoding: 'utf-8',
        timeout: 5000,
        maxBuffer: 1024 * 1024,
      }).trim();
      console.error(`✓ Retrieved ${taskLogs.split('\n').length} lines of logs`);
    } catch (err) {
      taskLogs = `Failed to retrieve logs: ${err.message}`;
      console.error(`✗ Failed to retrieve logs: ${err.message}`);
    }
  }
  console.error(`${'='.repeat(80)}\n`);

  agent._publish({
    topic: 'AGENT_ERROR',
    receiver: 'broadcast',
    content: {
      text: `Task logs for ${taskId} (last 100 lines)`,
      data: {
        taskId,
        logs: taskLogs,
        logsPreview: taskLogs.split('\n').slice(-20).join('\n'),
      },
    },
  });

  return taskId;
}

async function resolveResultData({ config, context, agent }) {
  const json = JSON.stringify(config);
  const usesResultVars = /\{\{result\.[^}]+\}\}/.test(json);

  if (!usesResultVars) return { json, resultData: null };

  if (!context.result) {
    throw new Error(
      `Hook uses result.* variables but no result in context. ` +
        `Agent: ${agent.id}, TaskID: ${agent.currentTaskId}, Iteration: ${agent.iteration}`
    );
  }

  if (!context.result.output) {
    const taskId = logEmptyOutputError({ agent, context, config });
    fetchAndPublishTaskLogs({ agent, taskId });
    throw new Error(
      `Hook uses result.* variables but result.output is empty. ` +
        `Agent: ${agent.id}, TaskID: ${taskId}, ` +
        `Iteration: ${context.result.iteration || agent.iteration}, ` +
        `Success: ${context.result.success}. Task logs posted to message bus.`
    );
  }

  const resultData = await agent._parseResultOutput(context.result.output);
  return { json, resultData };
}

/**
 * Substitute template variables in hook config.
 * ONLY parses result output if result.* variables are used.
 * THROWS on any error — no silent failures.
 */
async function substituteTemplate(params) {
  const { config, context, agent, cluster } = params;

  if (!config) {
    throw new Error('_substituteTemplate: config is required');
  }

  const { json, resultData } = await resolveResultData({ config, context, agent });

  let substituted = substituteStaticVars(json, context, agent, cluster);

  if (resultData) {
    substituted = substituteResultVars(substituted, resultData, agent);
  }

  checkUnsubstitutedVars(substituted);

  try {
    return JSON.parse(substituted);
  } catch (e) {
    console.error('JSON parse failed. Substituted string:');
    console.error(substituted);
    throw new Error(`Template substitution produced invalid JSON: ${e.message}`);
  }
}

module.exports = { substituteTemplate };
