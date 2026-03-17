const assert = require('node:assert');

const Ledger = require('../ledger');
const MessageBus = require('../message-bus');
const LogicEngine = require('../logic-engine');
const { executeHook } = require('../agent/agent-hook-executor');

function createSimAgent({ agentConfig, cluster, messageBus }) {
  const simAgent = {
    id: agentConfig.id,
    role: agentConfig.role,
    iteration: 1,
    cluster,
    messageBus,
    config: agentConfig,
    currentTaskId: 'sim-task',
    workingDirectory: process.cwd(),
    _log: () => {},
    _resolveProvider: () => 'claude',
    _parseResultOutput: (output) => {
      // Simulation: parse JSON directly without LLM reformatting
      // (parseResultOutput is async and calls reformatOutput which hangs in tests)
      if (!output || !output.trim()) {
        throw new Error('Task execution failed - no output');
      }
      const { extractJsonFromOutput } = require('../agent/output-extraction');
      const providerName = 'claude';
      const parsed = extractJsonFromOutput(output, providerName);
      if (!parsed) {
        // In simulation, output is always valid JSON from simulated agents
        // If extraction fails, try direct JSON.parse
        try {
          return JSON.parse(output);
        } catch {
          throw new Error(`Simulated agent output is not valid JSON: ${output.substring(0, 100)}`);
        }
      }
      return parsed;
    },
    _publish: (message) => {
      const receiver = message.receiver || 'broadcast';
      return messageBus.publish({
        ...message,
        receiver,
        cluster_id: cluster.id,
        sender: simAgent.id,
      });
    },
  };
  return simAgent;
}

function createCluster(id, config) {
  const agents = config.agents.map((agent) => ({
    ...agent,
    // Mirror runtime AgentWrapper shape so trigger scripts can inspect either
    // `candidate.hooks` (resolved config) or `candidate.config.hooks` (live cluster).
    config: agent,
  }));
  return {
    id,
    agents,
    getAgentsByRole: (role) => agents.filter((agent) => agent.role === role),
  };
}

function getCoordinator(config, templateName, triggerTopic) {
  const coordinator = config.agents.find((a) => a.id === 'consensus-coordinator');
  assert.ok(coordinator, `${templateName}: consensus-coordinator missing`);
  const trigger = coordinator.triggers.find((candidate) => candidate.topic === triggerTopic);
  assert.ok(trigger?.logic?.script, `${templateName}: coordinator trigger logic missing`);
  assert.ok(coordinator.hooks?.onComplete, `${templateName}: coordinator onComplete missing`);
  return { coordinator, trigger };
}

function createSimulationRuntime(cluster) {
  const ledger = new Ledger(':memory:');
  const messageBus = new MessageBus(ledger);
  const logicEngine = new LogicEngine(messageBus, cluster);
  return { ledger, messageBus, logicEngine };
}

function publishMessage({ messageBus, clusterId, topic, sender, timestamp, data }) {
  messageBus.publish({
    cluster_id: clusterId,
    topic,
    sender,
    timestamp,
    ...(data ? { content: { data } } : {}),
  });
}

function publishValidatorResults({ messageBus, clusterId, topic, now, results }) {
  results.forEach((result, index) => {
    publishMessage({
      messageBus,
      clusterId,
      topic,
      sender: result.sender,
      timestamp: now + (index + 1) * 10,
      data: result.data,
    });
  });
}

function ensureGateOpened({ logicEngine, script, clusterId, topic, templateName }) {
  const gateOk = logicEngine.evaluate(
    script,
    { id: 'consensus-coordinator', cluster_id: clusterId, iteration: 1 },
    { topic, cluster_id: clusterId }
  );
  return gateOk ? null : `${templateName}: gate did not open after both validators`;
}

async function executeCoordinatorCompletion({
  coordinator,
  cluster,
  messageBus,
  triggerTopic,
  allApproved,
}) {
  const simAgent = createSimAgent({ agentConfig: coordinator, cluster, messageBus });
  const triggeringMessage = messageBus.findLast({
    cluster_id: cluster.id,
    topic: triggerTopic,
  });

  await executeHook({
    hook: coordinator.hooks.onComplete,
    agent: simAgent,
    message: triggeringMessage,
    result: {
      output: JSON.stringify({ allApproved, summary: allApproved ? 'ok' : 'nope' }),
      success: true,
      taskId: 'sim-task',
    },
    messageBus,
    cluster,
  });
}

function findMessage(messageBus, clusterId, topic) {
  return messageBus.findLast({
    cluster_id: clusterId,
    topic,
  });
}

function validateAggregatedErrors(validationResult, expectedErrors, templateName) {
  const errors = validationResult.content?.data?.errors || [];
  const hasAllErrors = expectedErrors.every((error) => errors.includes(error));
  if (!hasAllErrors) {
    return `${templateName}: rejection did not aggregate validator errors`;
  }
  return null;
}

function validateQuickScenarioOutcome({ allApproved, messageBus, clusterId }) {
  const passed = findMessage(messageBus, clusterId, 'QUICK_VALIDATION_PASSED');
  if (allApproved) {
    return passed ? null : 'quick-validation: expected QUICK_VALIDATION_PASSED';
  }

  const validationResult = findMessage(messageBus, clusterId, 'VALIDATION_RESULT');
  if (!validationResult) {
    return 'quick-validation: expected VALIDATION_RESULT on rejection';
  }

  return validateAggregatedErrors(
    validationResult,
    ['req-error', 'code-error'],
    'quick-validation'
  );
}

async function runValidationScenario({
  cluster,
  coordinator,
  trigger,
  triggerTopic,
  startMessage,
  validatorResults,
  templateName,
  allApproved,
  validateOutcome,
}) {
  const { ledger, messageBus, logicEngine } = createSimulationRuntime(cluster);
  const now = Date.now();

  try {
    publishMessage({
      messageBus,
      clusterId: cluster.id,
      topic: startMessage.topic,
      sender: startMessage.sender,
      timestamp: now,
    });
    publishValidatorResults({
      messageBus,
      clusterId: cluster.id,
      topic: triggerTopic,
      now,
      results: validatorResults,
    });

    const gateFailure = ensureGateOpened({
      logicEngine,
      script: trigger.logic.script,
      clusterId: cluster.id,
      topic: triggerTopic,
      templateName,
    });
    if (gateFailure) {
      return gateFailure;
    }

    try {
      await executeCoordinatorCompletion({
        coordinator,
        cluster,
        messageBus,
        triggerTopic,
        allApproved,
      });
    } catch (err) {
      return `${templateName}: onComplete failed: ${err.message}`;
    }

    return validateOutcome({ allApproved, messageBus, clusterId: cluster.id });
  } finally {
    ledger.close();
  }
}

async function collectScenarioFailures(runScenario) {
  const failures = [];
  for (const allApproved of [true, false]) {
    const failure = await runScenario(allApproved);
    if (failure) {
      failures.push(failure);
    }
  }
  return failures;
}

async function simulateQuickValidation({ config }) {
  const cluster = createCluster('quick-sim', config);
  const { coordinator, trigger } = getCoordinator(
    config,
    'quick-validation',
    'QUICK_VALIDATION_RESULT'
  );

  const validators = cluster.agents
    .filter((agent) => {
      const hookTopic =
        agent?.config?.hooks?.onComplete?.config?.topic || agent?.hooks?.onComplete?.config?.topic;
      return agent.role === 'validator' && hookTopic === 'QUICK_VALIDATION_RESULT';
    })
    .map((agent) => agent.id);

  const errorsByValidator = {
    'validator-requirements': ['req-error'],
    'validator-code': ['code-error'],
  };

  const baseValidatorResults = validators
    .filter((id) => errorsByValidator[id])
    .map((id) => ({ sender: id, errors: errorsByValidator[id] }));

  const failures = await collectScenarioFailures((allApproved) => {
    const validatorResults = baseValidatorResults.map((result) => ({
      sender: result.sender,
      data: { approved: allApproved, errors: allApproved ? [] : result.errors },
    }));

    return runValidationScenario({
      cluster,
      coordinator,
      trigger,
      triggerTopic: 'QUICK_VALIDATION_RESULT',
      startMessage: { topic: 'IMPLEMENTATION_READY', sender: 'worker' },
      validatorResults,
      templateName: 'quick-validation',
      allApproved,
      validateOutcome: validateQuickScenarioOutcome,
    });
  });

  return { failures, validators };
}

async function simulateHeavyValidation({ config }) {
  const cluster = createCluster('heavy-sim', config);
  const { coordinator, trigger } = getCoordinator(
    config,
    'heavy-validation',
    'HEAVY_VALIDATION_RESULT'
  );

  const validators = cluster.agents
    .filter((agent) => {
      const hookTopic =
        agent?.config?.hooks?.onComplete?.config?.topic || agent?.hooks?.onComplete?.config?.topic;
      return agent.role === 'validator' && hookTopic === 'HEAVY_VALIDATION_RESULT';
    })
    .map((agent) => agent.id);

  // Build error map only for validators actually present in the resolved config
  const errorsByValidator = {};
  for (const validatorId of validators) {
    if (validatorId === 'validator-security') {
      errorsByValidator[validatorId] = ['sec-error'];
    } else if (validatorId === 'validator-tester') {
      errorsByValidator[validatorId] = ['test-error'];
    } else if (validatorId === 'validator-runtime') {
      errorsByValidator[validatorId] = ['runtime-error'];
    }
  }

  const validatorResults = validators
    .filter((id) => errorsByValidator[id])
    .map((id) => ({ sender: id, errors: errorsByValidator[id] }));

  const failures = await collectScenarioFailures((allApproved) => {
    const scenarioResults = validatorResults.map((result) => ({
      sender: result.sender,
      data: { approved: allApproved, errors: allApproved ? [] : result.errors },
    }));
    const expectedErrors = allApproved ? [] : validatorResults.map((r) => r.errors).flat();

    return runValidationScenario({
      cluster,
      coordinator,
      trigger,
      triggerTopic: 'HEAVY_VALIDATION_RESULT',
      startMessage: { topic: 'QUICK_VALIDATION_PASSED', sender: 'consensus-coordinator' },
      validatorResults: scenarioResults,
      templateName: 'heavy-validation',
      allApproved,
      validateOutcome: ({ messageBus, clusterId }) => {
        const validationResult = findMessage(messageBus, clusterId, 'VALIDATION_RESULT');
        if (!validationResult) {
          return 'heavy-validation: expected VALIDATION_RESULT';
        }
        return validateAggregatedErrors(validationResult, expectedErrors, 'heavy-validation');
      },
    });
  });

  return { failures, validators };
}

/**
 * Detect if config is a quick-validation structure by checking for consensus-coordinator
 * with trigger on QUICK_VALIDATION_RESULT
 */
function isQuickValidationConfig(config) {
  const agents = config?.agents || [];
  const coordinator = agents.find((a) => a.id === 'consensus-coordinator');
  if (!coordinator) return false;

  const triggers = coordinator.triggers || [];
  return triggers.some((t) => t.topic === 'QUICK_VALIDATION_RESULT');
}

/**
 * Detect if config is a heavy-validation structure by checking for consensus-coordinator
 * with trigger on HEAVY_VALIDATION_RESULT
 */
function isHeavyValidationConfig(config) {
  const agents = config?.agents || [];
  const coordinator = agents.find((a) => a.id === 'consensus-coordinator');
  if (!coordinator) return false;

  const triggers = coordinator.triggers || [];
  return triggers.some((t) => t.topic === 'HEAVY_VALIDATION_RESULT');
}

/**
 * Deep sim: run deterministic two-stage validation scenarios for base templates.
 * Detects validation configs by structure (presence of consensus-coordinator with
 * specific triggers), not just by templateId.
 * Returns Promise<{ failures: string[], validators?: string[] }>
 */
function simulateTwoStageValidation({ templateId, config }) {
  // Check templateId first for exact matches (fastest path)
  if (templateId === 'quick-validation') {
    return simulateQuickValidation({ config });
  }
  if (templateId === 'heavy-validation') {
    return simulateHeavyValidation({ config });
  }

  // Fall back to structural detection for configs that follow the pattern
  // but have different template IDs (e.g., 'quick-validation-test')
  if (isQuickValidationConfig(config)) {
    return simulateQuickValidation({ config });
  }
  if (isHeavyValidationConfig(config)) {
    return simulateHeavyValidation({ config });
  }

  return Promise.resolve({ failures: [], validators: [] });
}

module.exports = {
  simulateTwoStageValidation,
};
