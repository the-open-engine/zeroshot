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

async function simulateQuickValidation({ config }) {
  const cluster = {
    id: 'quick-sim',
    agents: config.agents.map((a) => ({ id: a.id, role: a.role })),
  };

  const coordinator = config.agents.find((a) => a.id === 'consensus-coordinator');
  assert.ok(coordinator, 'quick-validation: consensus-coordinator missing');

  const trigger = coordinator.triggers.find((t) => t.topic === 'QUICK_VALIDATION_RESULT');
  assert.ok(trigger?.logic?.script, 'quick-validation: coordinator trigger logic missing');
  assert.ok(coordinator.hooks?.onComplete, 'quick-validation: coordinator onComplete missing');

  const failures = [];

  const runScenario = async (allApproved) => {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const logicEngine = new LogicEngine(messageBus, cluster);

    // Stage start
    const now = Date.now();
    messageBus.publish({
      cluster_id: cluster.id,
      topic: 'IMPLEMENTATION_READY',
      sender: 'worker',
      timestamp: now,
    });

    // Validator outputs (stage 1)
    messageBus.publish({
      cluster_id: cluster.id,
      topic: 'QUICK_VALIDATION_RESULT',
      sender: 'validator-requirements',
      timestamp: now + 10,
      content: { data: { approved: true, errors: ['req-error'] } },
    });
    messageBus.publish({
      cluster_id: cluster.id,
      topic: 'QUICK_VALIDATION_RESULT',
      sender: 'validator-code',
      timestamp: now + 20,
      content: { data: { approved: true, errors: ['code-error'] } },
    });

    const gateOk = logicEngine.evaluate(
      trigger.logic.script,
      { id: 'consensus-coordinator', cluster_id: cluster.id },
      { topic: 'QUICK_VALIDATION_RESULT' }
    );
    if (!gateOk) {
      ledger.close();
      return { ok: false, error: 'quick-validation: gate did not open after both validators' };
    }

    const simAgent = createSimAgent({ agentConfig: coordinator, cluster, messageBus });
    const triggeringMessage = messageBus.findLast({
      cluster_id: cluster.id,
      topic: 'QUICK_VALIDATION_RESULT',
    });

    try {
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
    } catch (err) {
      ledger.close();
      return { ok: false, error: `quick-validation: onComplete failed: ${err.message}` };
    }

    const passed = messageBus.findLast({
      cluster_id: cluster.id,
      topic: 'QUICK_VALIDATION_PASSED',
    });
    const validationResult = messageBus.findLast({
      cluster_id: cluster.id,
      topic: 'VALIDATION_RESULT',
    });

    ledger.close();

    if (allApproved) {
      if (!passed)
        return { ok: false, error: 'quick-validation: expected QUICK_VALIDATION_PASSED' };
      return { ok: true };
    }

    if (!validationResult) {
      return { ok: false, error: 'quick-validation: expected VALIDATION_RESULT on rejection' };
    }

    const errors = validationResult.content?.data?.errors || [];
    if (!errors.includes('req-error') || !errors.includes('code-error')) {
      return { ok: false, error: 'quick-validation: rejection did not aggregate validator errors' };
    }

    return { ok: true };
  };

  for (const allApproved of [true, false]) {
    const res = await runScenario(allApproved);
    if (!res.ok) failures.push(res.error);
  }

  return failures;
}

async function simulateHeavyValidation({ config }) {
  const cluster = {
    id: 'heavy-sim',
    agents: config.agents.map((a) => ({ id: a.id, role: a.role })),
  };

  const coordinator = config.agents.find((a) => a.id === 'consensus-coordinator');
  assert.ok(coordinator, 'heavy-validation: consensus-coordinator missing');

  const trigger = coordinator.triggers.find((t) => t.topic === 'HEAVY_VALIDATION_RESULT');
  assert.ok(trigger?.logic?.script, 'heavy-validation: coordinator trigger logic missing');
  assert.ok(coordinator.hooks?.onComplete, 'heavy-validation: coordinator onComplete missing');

  const failures = [];

  const runScenario = async (allApproved) => {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const logicEngine = new LogicEngine(messageBus, cluster);

    const now = Date.now();
    messageBus.publish({
      cluster_id: cluster.id,
      topic: 'QUICK_VALIDATION_PASSED',
      sender: 'consensus-coordinator',
      timestamp: now,
    });

    messageBus.publish({
      cluster_id: cluster.id,
      topic: 'HEAVY_VALIDATION_RESULT',
      sender: 'validator-security',
      timestamp: now + 10,
      content: { data: { approved: true, errors: ['sec-error'] } },
    });
    messageBus.publish({
      cluster_id: cluster.id,
      topic: 'HEAVY_VALIDATION_RESULT',
      sender: 'validator-tester',
      timestamp: now + 20,
      content: { data: { approved: true, errors: ['test-error'] } },
    });

    const gateOk = logicEngine.evaluate(
      trigger.logic.script,
      { id: 'consensus-coordinator', cluster_id: cluster.id },
      { topic: 'HEAVY_VALIDATION_RESULT' }
    );
    if (!gateOk) {
      ledger.close();
      return { ok: false, error: 'heavy-validation: gate did not open after both validators' };
    }

    const simAgent = createSimAgent({ agentConfig: coordinator, cluster, messageBus });
    const triggeringMessage = messageBus.findLast({
      cluster_id: cluster.id,
      topic: 'HEAVY_VALIDATION_RESULT',
    });

    try {
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
    } catch (err) {
      ledger.close();
      return { ok: false, error: `heavy-validation: onComplete failed: ${err.message}` };
    }

    const validationResult = messageBus.findLast({
      cluster_id: cluster.id,
      topic: 'VALIDATION_RESULT',
    });
    ledger.close();

    if (!validationResult) {
      return { ok: false, error: 'heavy-validation: expected VALIDATION_RESULT' };
    }

    const errors = validationResult.content?.data?.errors || [];
    if (!errors.includes('sec-error') || !errors.includes('test-error')) {
      return { ok: false, error: 'heavy-validation: did not aggregate validator errors' };
    }

    return { ok: true };
  };

  for (const allApproved of [true, false]) {
    const res = await runScenario(allApproved);
    if (!res.ok) failures.push(res.error);
  }

  return failures;
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
