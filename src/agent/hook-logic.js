/**
 * Hook Logic - Evaluate logic scripts for conditional hook config overrides
 */

const vm = require('vm');
const { buildSandbox } = require('./hook-sandbox');

/**
 * Evaluate hook logic script to get config overrides.
 * Similar to trigger logic, but returns an object to merge into config.
 * @param {Object} params - Evaluation parameters
 * @param {Object} params.logic - Logic configuration { engine, script }
 * @param {Object} params.resultData - Parsed agent result data
 * @param {Object} params.agent - Agent instance
 * @param {Object} params.context - Execution context
 * @returns {Object|null} Config overrides to merge, or null if none
 */
function evaluateHookLogic(params) {
  const { logic, resultData, agent, context } = params;

  if (!logic || !logic.script) {
    return null;
  }

  if (logic.engine !== 'javascript') {
    throw new Error(`Unsupported hook logic engine: ${logic.engine}`);
  }

  const sandbox = buildSandbox({ agent, context, resultData, logPrefix: '[hook-logic]' });

  // Hook logic also exposes agent metadata and message alias
  sandbox.agent = {
    id: agent.id,
    role: agent.role,
    iteration: agent.iteration || 0,
  };
  sandbox.message = context.triggeringMessage || null;

  const vmContext = vm.createContext(sandbox);
  const wrappedScript = `(function() { 'use strict'; ${logic.script} })()`;

  let result;
  try {
    result = vm.runInContext(wrappedScript, vmContext, { timeout: 1000 });
  } catch (err) {
    throw new Error(`Hook logic script error: ${err.message}`);
  }

  if (result === undefined || result === null) {
    return null;
  }

  if (typeof result !== 'object') {
    throw new Error(`Hook logic script must return an object or undefined, got: ${typeof result}`);
  }

  return result;
}

module.exports = { evaluateHookLogic };
