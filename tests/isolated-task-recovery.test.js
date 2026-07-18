const assert = require('assert');

const { startLivenessCheck, stopLivenessCheck } = require('../src/agent/agent-lifecycle');
const { followClaudeTaskLogsIsolated } = require('../src/agent/agent-task-executor');
const {
  createIsolatedAgent,
  createIsolationManager,
  runWatchdogRecovery,
  useZeroBackoffSettings,
  waitFor,
} = require('./helpers/isolated-task-recovery-fixture');

describe('Isolated task recovery', function () {
  this.timeout(7000);
  useZeroBackoffSettings();

  it('terminates a stale task inside its container exactly once', async function () {
    const recovered = await runWatchdogRecovery({
      lastOutputTime: Date.now() - 100,
      taskStartedAt: Date.now() - 100,
    });

    assert.strictEqual(recovered.manager.killCalls, 1);
    assert.strictEqual(recovered.result.success, false);
    assert.strictEqual(recovered.result.code, 'PROVIDER_INACTIVITY_TIMEOUT');
    assert.strictEqual(recovered.agent.currentTask, null);
    assert.strictEqual(recovered.agent.currentTaskId, null);
    assert.ok(
      recovered.events.some(({ event }) => event === 'AGENT_INACTIVITY_TIMEOUT'),
      'durable inactivity recovery event should be emitted'
    );
  });

  it('terminates an absolute-timeout task inside its container exactly once', async function () {
    const recovered = await runWatchdogRecovery({
      timeout: 30,
      staleDuration: 1000,
      lastOutputTime: Date.now(),
      taskStartedAt: Date.now() - 100,
    });

    assert.strictEqual(recovered.manager.killCalls, 1);
    assert.strictEqual(recovered.result.success, false);
    assert.strictEqual(recovered.result.code, 'AGENT_TASK_TIMEOUT');
    assert.strictEqual(recovered.agent.currentTask, null);
    assert.strictEqual(recovered.agent.currentTaskId, null);
    assert.ok(
      recovered.events.some(({ event }) => event === 'AGENT_TASK_TIMEOUT'),
      'durable absolute-timeout recovery event should be emitted'
    );
  });

  it('re-arms lifecycle recovery after an in-container kill command fails', async function () {
    const recovered = await runWatchdogRecovery(
      {
        lastOutputTime: Date.now() - 100,
        taskStartedAt: Date.now() - 100,
      },
      { killCommandFailures: 1 }
    );

    assert.strictEqual(recovered.manager.killCalls, 2);
    assert.strictEqual(recovered.manager.maxConcurrentKillCalls, 1);
    assert.strictEqual(recovered.result.success, false);
    assert.strictEqual(recovered.agent.currentTask, null);
    assert.strictEqual(recovered.agent.currentTaskId, null);
  });

  it('re-arms lifecycle recovery when kill status cannot be verified', async function () {
    const recovered = await runWatchdogRecovery(
      {
        lastOutputTime: Date.now() - 100,
        taskStartedAt: Date.now() - 100,
      },
      { unverifiableKillAttempts: 1 }
    );

    assert.strictEqual(recovered.manager.killCalls, 2);
    assert.strictEqual(recovered.manager.maxConcurrentKillCalls, 1);
    assert.strictEqual(recovered.result.success, false);
    assert.strictEqual(recovered.agent.currentTask, null);
    assert.strictEqual(recovered.agent.currentTaskId, null);
  });
});

describe('Isolated task lifecycle handles', function () {
  this.timeout(7000);
  useZeroBackoffSettings();

  it('does not ignore a tracked isolated task while its local handle is briefly absent', async function () {
    const manager = createIsolationManager();
    const { agent, events } = createIsolatedAgent(manager, {
      currentTask: null,
      lastOutputTime: Date.now() - 100,
      taskStartedAt: Date.now() - 100,
    });
    let termination = null;
    agent._killTask = (reason) => {
      termination = reason;
      agent.currentTaskId = null;
    };

    startLivenessCheck(agent);
    await waitFor(() => termination);
    stopLivenessCheck(agent);

    assert.strictEqual(termination.code, 'PROVIDER_INACTIVITY_TIMEOUT');
    assert.ok(events.some(({ event }) => event === 'AGENT_INACTIVITY_TIMEOUT'));
  });

  it('does not kill or retry a task that completed before recovery reconciliation', async function () {
    const manager = createIsolationManager({ status: 'completed' });
    const { agent } = createIsolatedAgent(manager, {
      lastOutputTime: Date.now() - 100,
      taskStartedAt: Date.now() - 100,
    });
    const execution = followClaudeTaskLogsIsolated(agent, agent.currentTaskId);

    await waitFor(() => agent.currentTask);
    startLivenessCheck(agent);
    const result = await execution;
    stopLivenessCheck(agent);

    assert.strictEqual(result.success, true);
    assert.strictEqual(manager.killCalls, 0);
    assert.strictEqual(agent.currentTask, null);
  });

  it('clears the lifecycle handle on normal isolated completion without recovery', async function () {
    const manager = createIsolationManager({ status: 'completed' });
    const { agent } = createIsolatedAgent(manager);
    const result = await followClaudeTaskLogsIsolated(agent, agent.currentTaskId);

    assert.strictEqual(result.success, true);
    assert.strictEqual(manager.killCalls, 0);
    assert.strictEqual(agent.currentTask, null);
  });

  it('clears the lifecycle handle when isolated log setup fails', async function () {
    const manager = createIsolationManager();
    manager.execInContainer = () =>
      Promise.resolve({ code: 1, stdout: '', stderr: 'log lookup failed' });
    const { agent } = createIsolatedAgent(manager);

    await assert.rejects(
      followClaudeTaskLogsIsolated(agent, agent.currentTaskId),
      /log lookup failed/
    );
    assert.strictEqual(agent.currentTask, null);
  });
});
