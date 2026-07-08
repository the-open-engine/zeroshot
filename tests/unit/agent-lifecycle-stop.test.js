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
});
