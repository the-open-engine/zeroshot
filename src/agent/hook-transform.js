/**
 * Hook Transform - Transform script execution in VM sandbox
 */

const vm = require('vm');
const { buildSandbox } = require('./hook-sandbox');

function getAccessedFields(script) {
  return [...script.matchAll(/result\.([a-zA-Z_]+)/g)].map((m) => m[1]);
}

function logTransformParseFailure({ agent, context, parseError }) {
  const taskId = context.result?.taskId || agent.currentTaskId || 'UNKNOWN';
  console.error(`\n${'='.repeat(80)}`);
  console.error(`🔴 TRANSFORM SCRIPT BLOCKED - RESULT PARSING FAILED`);
  console.error(`${'='.repeat(80)}`);
  console.error(`Agent: ${agent.id}, Role: ${agent.role}`);
  console.error(`TaskID: ${taskId}`);
  console.error(`Parse error: ${parseError.message}`);
  console.error(`Output (last 500 chars): ${(context.result.output || '').slice(-500)}`);
  console.error(`${'='.repeat(80)}\n`);
}

function logMissingResultFields({ agent, context, accessedFields, missingFields, resultData }) {
  const taskId = context.result?.taskId || agent.currentTaskId || 'UNKNOWN';
  console.error(`\n${'='.repeat(80)}`);
  console.error(`🔴 TRANSFORM SCRIPT BLOCKED - MISSING REQUIRED FIELDS`);
  console.error(`${'='.repeat(80)}`);
  console.error(`Agent: ${agent.id}, Role: ${agent.role}, TaskID: ${taskId}`);
  console.error(`Script accesses: ${accessedFields.join(', ')}`);
  console.error(`Missing from result: ${missingFields.join(', ')}`);
  console.error(`Result keys: ${Object.keys(resultData).join(', ')}`);
  console.error(`Result data: ${JSON.stringify(resultData, null, 2)}`);
  console.error(`${'='.repeat(80)}\n`);
}

async function parseResultWithValidation({ context, agent, script }) {
  let resultData = null;
  try {
    resultData = await agent._parseResultOutput(context.result.output);
  } catch (parseError) {
    logTransformParseFailure({ agent, context, parseError });
    throw new Error(
      `Transform script cannot run: result parsing failed. ` +
        `Agent: ${agent.id}, Error: ${parseError.message}`
    );
  }

  const accessedFields = getAccessedFields(script);
  const missingFields = accessedFields.filter((f) => resultData[f] === undefined);
  if (missingFields.length > 0) {
    logMissingResultFields({ agent, context, accessedFields, missingFields, resultData });
    const taskId = context.result?.taskId || agent.currentTaskId || 'UNKNOWN';
    throw new Error(
      `Transform script accesses undefined fields: ${missingFields.join(', ')}. ` +
        `Agent ${agent.id} (task ${taskId}) output missing required fields. ` +
        `Check agent's jsonSchema and output format.`
    );
  }

  return resultData;
}

function parseTransformResultData({ context, agent, script, scriptUsesResult }) {
  if (context.result?.output) {
    return parseResultWithValidation({ context, agent, script });
  }

  if (scriptUsesResult) {
    const taskId = context.result?.taskId || agent.currentTaskId || 'UNKNOWN';
    const outputLength = (context.result?.output || '').length;
    throw new Error(
      `Transform script uses result.* variables but no output was captured. ` +
        `Agent: ${agent.id}, TaskID: ${taskId}, Iteration: ${agent.iteration}, ` +
        `Output length: ${outputLength}. ` +
        `Check that the task completed successfully and the get-log-path command exists.`
    );
  }

  return null;
}

function buildTransformSandbox({ resultData, context, agent }) {
  const sandbox = buildSandbox({ agent, context, resultData, logPrefix: '[transform]' });
  // Transform sandbox uses 'result' directly (not wrapped in agent context)
  sandbox.result = resultData;
  sandbox.triggeringMessage = context.triggeringMessage;
  return sandbox;
}

function runTransformScript(script, sandbox) {
  const vmContext = vm.createContext(sandbox);
  const wrappedScript = `(function() { ${script} })()`;

  try {
    return vm.runInContext(wrappedScript, vmContext, { timeout: 5000 });
  } catch (err) {
    throw new Error(`Transform script error: ${err.message}`);
  }
}

function validateTransformResult(result) {
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
}

function validateClusterOperationsResult(result, agent) {
  if (result.topic !== 'CLUSTER_OPERATIONS') return;

  const operations = result.content?.data?.operations;
  if (!operations) {
    console.error(`\n${'='.repeat(80)}`);
    console.error(`🔴 CLUSTER_OPERATIONS MALFORMED - MISSING OPERATIONS ARRAY`);
    console.error(`${'='.repeat(80)}`);
    console.error(`Agent: ${agent.id}`);
    console.error(`Result: ${JSON.stringify(result, null, 2)}`);
    console.error(`${'='.repeat(80)}\n`);
    throw new Error(
      `CLUSTER_OPERATIONS message missing operations array. ` +
        `Agent ${agent.id} transform script returned invalid structure.`
    );
  }
  if (!Array.isArray(operations)) {
    throw new Error(`CLUSTER_OPERATIONS.operations must be an array, got: ${typeof operations}`);
  }
  if (operations.length === 0) {
    throw new Error(`CLUSTER_OPERATIONS.operations is empty - no operations to execute`);
  }

  for (let i = 0; i < operations.length; i++) {
    const op = operations[i];
    if (!op || !op.action) {
      throw new Error(`CLUSTER_OPERATIONS.operations[${i}] missing required 'action' field`);
    }
  }

  agent._log(`✅ CLUSTER_OPERATIONS validated: ${operations.length} operations`);
}

/**
 * Execute a hook transform script
 * Transform scripts return the message to publish, with access to:
 * - result: parsed agent output
 * - triggeringMessage: the message that triggered the agent
 * - helpers: { getConfig(complexity, taskType) }
 */
async function executeTransform(params) {
  const { transform, context, agent } = params;
  const { engine, script } = transform;

  if (engine !== 'javascript') {
    throw new Error(`Unsupported transform engine: ${engine}`);
  }

  const scriptUsesResult = /\bresult\.[a-zA-Z]/.test(script);
  const resultData = await parseTransformResultData({
    context,
    agent,
    script,
    scriptUsesResult,
  });

  const sandbox = buildTransformSandbox({ resultData, context, agent });
  const result = runTransformScript(script, sandbox);

  validateTransformResult(result);
  validateClusterOperationsResult(result, agent);

  return result;
}

module.exports = { executeTransform };
