const assert = require('assert');
const { runDetachedRecovery, runInProcessRecovery } = require('../helpers/stuck-recovery-fixture');

function eventCount(result, event) {
  return result.lifecycle.filter((candidate) => candidate === event).length;
}

describe('e2e: stuck agent recovery', function () {
  this.timeout(30000);

  it('recovers a normal-mode fake Codex provider stalled after turn.started', async function () {
    const result = await runInProcessRecovery();
    assert.strictEqual(result.state, 'stopped');
    assert.strictEqual(eventCount(result, 'AGENT_INACTIVITY_TIMEOUT'), 1);
    assert.strictEqual(eventCount(result, 'AGENT_RESTART_ATTEMPT'), 1);
    assert.strictEqual(eventCount(result, 'TASK_COMPLETED'), 1);
    assert.strictEqual(result.agentState.currentTask, false);
    assert.strictEqual(result.agentState.currentTaskId, null);
    assert.strictEqual(result.agentState.processPid, null);
    assert.strictEqual(result.fakeCount, '2');
  });

  it('recovers in a detached daemon and reconciles external provider death', async function () {
    const result = await runDetachedRecovery();
    assert.deepStrictEqual([result.state, result.pid, result.fakeCount], ['stopped', null, '3']);
    assert.strictEqual(eventCount(result, 'AGENT_INACTIVITY_TIMEOUT'), 1);
    assert.strictEqual(eventCount(result, 'AGENT_RESTART_ATTEMPT'), 1);
    assert.strictEqual(eventCount(result, 'TASK_FAILED'), 2);
    assert.strictEqual(eventCount(result, 'RETRY_SCHEDULED'), 2);
    assert.strictEqual(eventCount(result, 'TASK_COMPLETED'), 1);
  });
});
