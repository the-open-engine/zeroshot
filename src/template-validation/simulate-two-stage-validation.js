const assert = require('node:assert');

const Ledger = require('../ledger');
const MessageBus = require('../message-bus');
const LogicEngine = require('../logic-engine');
const { executeHook } = require('../agent/agent-hook-executor');
const { parseResultOutput } = require('../agent/agent-task-executor');

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
    _parseResultOutput: (output) => parseResultOutput(simAgent, output),
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
  const agents = config.agents.map((agent) => ({ id: agent.id, role: agent.role }));
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
    { id: 'consensus-coordinator', cluster_id: clusterId },
    { topic }
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

function simulateQuickValidation({ config }) {
  const cluster = createCluster('quick-sim', config);
  const { coordinator, trigger } = getCoordinator(
    config,
    'quick-validation',
    'QUICK_VALIDATION_RESULT'
  );
  const validatorResults = [
    { sender: 'validator-requirements', data: { approved: true, errors: ['req-error'] } },
    { sender: 'validator-code', data: { approved: true, errors: ['code-error'] } },
  ];

  return collectScenarioFailures((allApproved) =>
    runValidationScenario({
      cluster,
      coordinator,
      trigger,
      triggerTopic: 'QUICK_VALIDATION_RESULT',
      startMessage: { topic: 'IMPLEMENTATION_READY', sender: 'worker' },
      validatorResults,
      templateName: 'quick-validation',
      allApproved,
      validateOutcome: validateQuickScenarioOutcome,
    })
  );
}

function simulateHeavyValidation({ config }) {
  const cluster = createCluster('heavy-sim', config);
  const { coordinator, trigger } = getCoordinator(
    config,
    'heavy-validation',
    'HEAVY_VALIDATION_RESULT'
  );

  const validators = cluster.agents
    .filter((agent) => agent.role === 'validator')
    .map((agent) => agent.id);

  const allPossibleResults = [
    { sender: 'validator-security', data: { approved: true, errors: ['sec-error'] } },
    { sender: 'validator-tester', data: { approved: true, errors: ['test-error'] } },
    { sender: 'validator-runtime', data: { approved: true, errors: ['runtime-error'] } },
  ];

  const validatorResults = allPossibleResults.filter((result) =>
    validators.includes(result.sender)
  );

  const expectedErrors = validatorResults.map((result) => result.data.errors).flat();

  return collectScenarioFailures((allApproved) =>
    runValidationScenario({
      cluster,
      coordinator,
      trigger,
      triggerTopic: 'HEAVY_VALIDATION_RESULT',
      startMessage: { topic: 'QUICK_VALIDATION_PASSED', sender: 'consensus-coordinator' },
      validatorResults,
      templateName: 'heavy-validation',
      allApproved,
      validateOutcome: ({ messageBus, clusterId }) => {
        const validationResult = findMessage(messageBus, clusterId, 'VALIDATION_RESULT');
        if (!validationResult) {
          return 'heavy-validation: expected VALIDATION_RESULT';
        }
        return validateAggregatedErrors(validationResult, expectedErrors, 'heavy-validation');
      },
    })
  );
}

/**
 * Deep sim: run deterministic two-stage validation scenarios for base templates.
 * Returns an array of error strings.
 */
function simulateTwoStageValidation({ templateId, config }) {
  if (templateId === 'quick-validation') {
    return simulateQuickValidation({ config });
  }
  if (templateId === 'heavy-validation') {
    return simulateHeavyValidation({ config });
  }
  return Promise.resolve([]);
}

module.exports = {
  simulateTwoStageValidation,
};
