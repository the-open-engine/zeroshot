const assert = require('assert');
const { PassThrough } = require('stream');

const { startLivenessCheck, stopLivenessCheck } = require('../src/agent/agent-lifecycle');
const { followClaudeTaskLogsIsolated, killTask } = require('../src/agent/agent-task-executor');

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function waitFor(predicate, timeoutMs = 1000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await sleep(5);
  }
  throw new Error('Timed out waiting for condition');
}

function createFakeProcess() {
  return {
    pid: 9012,
    stdout: new PassThrough(),
    stderr: new PassThrough(),
    kill() {},
    on() {},
  };
}

function createIsolationManager({ status = 'running' } = {}) {
  const commands = [];
  let taskStatus = status;
  return {
    commands,
    killCalls: 0,
    setStatus(nextStatus) {
      taskStatus = nextStatus;
    },
    spawnInContainer() {
      return createFakeProcess();
    },
    async execInContainer(_clusterId, command) {
      commands.push(command);
      const commandText = command.join(' ');
      if (commandText.includes('get-log-path')) {
        return { code: 0, stdout: '/tmp/provider.log\n', stderr: '' };
      }
      if (commandText.includes('kill')) {
        this.killCalls += 1;
        taskStatus = 'killed';
        await sleep(10);
        return { code: 0, stdout: 'killed\n', stderr: '' };
      }
      if (commandText.includes('status')) {
        return {
          code: 0,
          stdout: `Status: ${taskStatus}\n`,
          stderr: '',
        };
      }
      if (commandText.includes('cat')) {
        return { code: 0, stdout: '{"summary":"done","result":"ok"}\n', stderr: '' };
      }
      throw new Error(`Unexpected isolated command: ${command.join(' ')}`);
    },
  };
}

function createIsolatedAgent(manager, overrides = {}) {
  const events = [];
  const agent = {
    id: 'isolated-worker',
    role: 'implementation',
    isolation: {
      enabled: true,
      manager,
      clusterId: 'isolated-cluster',
    },
    cluster: { id: 'cluster' },
    config: { cwd: '/tmp/work' },
    worktree: null,
    iteration: 1,
    timeout: 0,
    staleDuration: 20,
    enableLivenessCheck: true,
    currentTask: null,
    currentTaskId: 'isolated-task',
    processPid: 9012,
    lastOutputTime: Date.now(),
    taskStartedAt: Date.now(),
    livenessCheckInterval: null,
    livenessTerminationStarted: false,
    consecutiveStaleWarnings: 0,
    messageBus: { publish() {} },
    _resolveProvider: () => 'codex',
    _parseResultOutput: () => Promise.resolve({ summary: 'done', result: 'ok' }),
    _log() {},
    _publishLifecycle(event, data) {
      events.push({ event, data });
    },
    _stopLivenessCheck() {
      stopLivenessCheck(this);
    },
    _killTask(termination) {
      return killTask(this, termination);
    },
    ...overrides,
  };
  return { agent, events };
}

async function runWatchdogRecovery(overrides) {
  const manager = createIsolationManager();
  const { agent, events } = createIsolatedAgent(manager, overrides);
  const execution = followClaudeTaskLogsIsolated(agent, agent.currentTaskId);

  await waitFor(() => agent.currentTask);
  startLivenessCheck(agent);
  const result = await execution;
  await sleep(80);
  stopLivenessCheck(agent);

  return { agent, events, manager, result };
}

describe('Isolated task recovery', function () {
  this.timeout(7000);

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
