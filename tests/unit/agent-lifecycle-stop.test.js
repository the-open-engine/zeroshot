const assert = require('node:assert');

const { stop } = require('../../src/agent/agent-lifecycle');

describe('Agent lifecycle stop', function () {
  it('clears liveness monitoring even when the agent is already not running', async function () {
    const interval = setInterval(() => {}, 60_000);
    const agent = {
      running: false,
      livenessCheckInterval: interval,
    };

    await stop(agent);

    assert.strictEqual(agent.livenessCheckInterval, null);
  });

  it('cancels the bounded-wait timer after in-flight execution settles', async function () {
    const originalSetTimeout = global.setTimeout;
    const originalClearTimeout = global.clearTimeout;
    const timeoutHandle = {};
    let clearedHandle = null;
    global.setTimeout = (callback, delay) => {
      assert.strictEqual(delay, 5000);
      return timeoutHandle;
    };
    global.clearTimeout = (handle) => {
      clearedHandle = handle;
    };

    try {
      const agent = {
        running: true,
        currentTask: null,
        _currentExecution: Promise.resolve(),
        unsubscribe: null,
        _log() {},
      };

      await stop(agent);

      assert.strictEqual(clearedHandle, timeoutHandle);
      assert.strictEqual(agent._currentExecution, null);
    } finally {
      global.setTimeout = originalSetTimeout;
      global.clearTimeout = originalClearTimeout;
    }
  });
});
