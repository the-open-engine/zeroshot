const assert = require('assert');

const {
  executeTask,
  startLivenessCheck,
  stopLivenessCheck,
} = require('../src/agent/agent-lifecycle');
const { followClaudeTaskLogsIsolated } = require('../src/agent/agent-task-executor');
const {
  createIsolatedAgent,
  createIsolationManager,
  sleep,
  useZeroBackoffSettings,
  waitFor,
} = require('./helpers/isolated-task-recovery-fixture');

async function runPermanentWatchdogFailure(managerOptions) {
  const manager = createIsolationManager(managerOptions);
  const { agent, events } = createIsolatedAgent(manager, {
    lastOutputTime: Date.now() - 100,
    taskStartedAt: Date.now() - 100,
  });
  const execution = followClaudeTaskLogsIsolated(agent, agent.currentTaskId);

  await waitFor(() => agent.currentTask);
  startLivenessCheck(agent);
  const outcome = await Promise.race([
    execution.then(
      (result) => ({ result }),
      (error) => ({ error })
    ),
    sleep(300).then(() => ({ timedOut: true })),
  ]);
  const observed = {
    agent,
    events,
    manager,
    outcome,
    killCalls: manager.killCalls,
    stillMonitored: Boolean(agent.livenessCheckInterval),
  };

  if (outcome.timedOut) {
    stopLivenessCheck(agent);
    manager.allowTermination();
    await agent.currentTask?.terminate('test cleanup');
    await execution;
  }

  return observed;
}

async function runPermanentLifecycleFailure(managerOptions) {
  const manager = createIsolationManager(managerOptions);
  const { agent, events } = createIsolatedAgent(manager, {
    config: { cwd: '/tmp/work', hooks: {}, maxRetries: 3 },
    currentTaskId: null,
    iteration: 0,
    maxIterations: 10,
    running: true,
    state: 'idle',
    testMode: true,
    quiet: true,
    _buildContext: () => 'task context',
    _selectModel: () => 'test-model',
    _resolveModelSpec: () => null,
  });
  const published = [];
  let spawnCalls = 0;
  agent._publish = (message) => published.push(message);
  agent._spawnClaudeTask = async function () {
    spawnCalls += 1;
    this.currentTaskId = `isolated-task-${spawnCalls}`;
    this.lastOutputTime = Date.now() - 100;
    this.taskStartedAt = Date.now() - 100;
    const execution = followClaudeTaskLogsIsolated(this, this.currentTaskId);
    await waitFor(() => this.currentTask);
    startLivenessCheck(this);
    return execution;
  };

  await executeTask(agent, { topic: 'ISSUE_OPENED', sender: 'system' });
  return { agent, events, manager, published, spawnCalls };
}

describe('Bounded isolated task recovery', function () {
  this.timeout(7000);
  useZeroBackoffSettings();

  const permanentFailures = [
    ['kill command failure', { killCommandFailures: Infinity }],
    ['status verification failure', { unverifiableKillAttempts: Infinity }],
  ];

  for (const [label, managerOptions] of permanentFailures) {
    it(`fails closed after bounded ${label} attempts`, async function () {
      const recovered = await runPermanentWatchdogFailure(managerOptions);

      assert.strictEqual(recovered.outcome.timedOut, undefined);
      assert.strictEqual(recovered.outcome.error?.code, 'ISOLATED_TASK_TERMINATION_EXHAUSTED');
      assert.strictEqual(recovered.outcome.error?.permanent, true);
      assert.strictEqual(recovered.killCalls, 3);
      assert.strictEqual(recovered.manager.maxConcurrentKillCalls, 1);
      assert.strictEqual(recovered.stillMonitored, false);
      assert.strictEqual(recovered.agent.currentTask, null);
      assert.strictEqual(recovered.agent.currentTaskId, 'isolated-task');
      assert.strictEqual(recovered.agent.state, 'error');
      assert.strictEqual(recovered.agent.cluster.failureInfo.type, 'task_termination');
      assert.strictEqual(recovered.agent.cluster.failureInfo.reason, 'termination_unverified');
      assert.strictEqual(recovered.agent.cluster.failureInfo.attempts, 3);
      assert.strictEqual(
        recovered.events.filter(({ event }) => event === 'AGENT_INACTIVITY_TIMEOUT').length,
        1
      );
      assert.strictEqual(
        recovered.events.filter(({ event }) => event === 'AGENT_TERMINATION_RETRY').length,
        2
      );
      assert.strictEqual(
        recovered.events.filter(({ event }) => event === 'AGENT_TERMINATION_EXHAUSTED').length,
        1
      );

      await sleep(50);
      assert.strictEqual(recovered.manager.killCalls, 3);
    });

    it(`does not retry the provider after permanent ${label}`, async function () {
      const recovered = await runPermanentLifecycleFailure(managerOptions);
      const agentErrors = recovered.published.filter(({ topic }) => topic === 'AGENT_ERROR');
      const clusterFailures = recovered.published.filter(({ topic }) => topic === 'CLUSTER_FAILED');

      assert.strictEqual(recovered.spawnCalls, 1);
      assert.strictEqual(recovered.manager.killCalls, 3);
      assert.strictEqual(recovered.manager.maxConcurrentKillCalls, 1);
      assert.strictEqual(recovered.agent.state, 'error');
      assert.strictEqual(recovered.agent.currentTaskId, 'isolated-task-1');
      assert.strictEqual(recovered.agent.cluster.failureInfo.type, 'task_termination');
      assert.strictEqual(recovered.agent.cluster.failureInfo.reason, 'termination_unverified');
      assert.strictEqual(recovered.agent.cluster.failureInfo.attempts, 3);
      assert.strictEqual(agentErrors.length, 1);
      assert.strictEqual(agentErrors[0].content.data.terminationExhausted, true);
      assert.strictEqual(agentErrors[0].content.data.restartExhausted, true);
      assert.strictEqual(agentErrors[0].content.data.attempts, 3);
      assert.strictEqual(clusterFailures.length, 1);
      assert.strictEqual(clusterFailures[0].content.data.reason, 'task_termination_unverified');
    });
  }
});
