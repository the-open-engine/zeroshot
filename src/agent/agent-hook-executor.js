/**
 * AgentHookExecutor - Hook dispatch and orchestration
 *
 * Delegates to:
 * - hook-transform.js: Transform script execution
 * - hook-template.js: Template variable substitution
 * - hook-logic.js: Logic scripts for conditional config
 * - pr-verification.js: GitHub PR verification
 * - hook-sandbox.js: Shared VM sandbox builder
 */

const { executeTransform } = require('./hook-transform');
const { substituteTemplate } = require('./hook-template');
const { evaluateHookLogic } = require('./hook-logic');
const { verifyGithubPr } = require('./pr-verification');

function isPlainObject(val) {
  return val !== null && typeof val === 'object' && !Array.isArray(val);
}

/**
 * Deep merge two objects, with source taking precedence.
 */
function deepMerge(target, source) {
  if (!isPlainObject(source)) return target;
  if (!isPlainObject(target)) return source;

  const result = { ...target };
  for (const key of Object.keys(source)) {
    result[key] =
      isPlainObject(source[key]) && isPlainObject(target[key])
        ? deepMerge(target[key], source[key])
        : source[key];
  }
  return result;
}

async function parseResultDataForHookLogic({ agent, result }) {
  if (!result?.output) return null;
  try {
    return await agent._parseResultOutput(result.output);
  } catch (parseError) {
    agent._log(
      `⚠️  Hook logic: result parsing failed, continuing with null: ${parseError.message}`
    );
    return null;
  }
}

async function resolveHookConfigWithLogic({ hook, agent, context, result }) {
  if (!hook.logic) return hook.config;

  const resultData = await parseResultDataForHookLogic({ agent, result });
  const overrides = evaluateHookLogic({
    logic: hook.logic,
    resultData,
    agent,
    context,
  });

  if (!overrides) return hook.config;

  agent._log(`Hook logic returned overrides: ${JSON.stringify(overrides)}`);
  return deepMerge(hook.config, overrides);
}

async function resolvePublishMessage({ hook, agent, context, cluster, result }) {
  if (hook.transform) {
    return executeTransform({ transform: hook.transform, context, agent });
  }

  const effectiveConfig = await resolveHookConfigWithLogic({ hook, agent, context, result });
  return substituteTemplate({ config: effectiveConfig, context, agent, cluster });
}

/**
 * Execute a hook. THROWS on failure — no silent errors.
 */
async function executeHook(params) {
  const { hook, agent, message, result, cluster } = params;

  if (!hook) return;

  const context = { result, triggeringMessage: message, agent, cluster };

  if (hook.action === 'publish_message') {
    const messageToPublish = await resolvePublishMessage({
      hook,
      agent,
      context,
      cluster,
      result,
    });
    agent._publish(messageToPublish);
    return;
  }

  if (hook.action === 'execute_system_command') {
    throw new Error('execute_system_command not implemented');
  }

  if (hook.action === 'verify_github_pr') {
    await verifyGithubPr({ result, agent });
    return;
  }

  throw new Error(`Unknown hook action: ${hook.action}`);
}

module.exports = {
  executeHook,
  executeTransform,
  substituteTemplate,
  evaluateHookLogic,
  deepMerge,
};
