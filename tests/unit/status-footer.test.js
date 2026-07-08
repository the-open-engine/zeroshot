const assert = require('assert');
const { StatusFooter, AGENT_STATE } = require('../../src/status-footer');

describe('StatusFooter updateAgent', () => {
  it('maps legacy pid to processPid', () => {
    const footer = new StatusFooter({ enabled: false });
    footer.updateAgent({
      id: 'worker',
      state: AGENT_STATE.EXECUTING_TASK,
      pid: 1234,
      iteration: 1,
    });

    const agent = footer.agents.get('worker');
    assert.strictEqual(agent.processPid, 1234);
    assert.strictEqual(agent.pid, 1234);
  });

  it('prefers explicit processPid over pid', () => {
    const footer = new StatusFooter({ enabled: false });
    footer.updateAgent({
      id: 'worker',
      state: AGENT_STATE.EXECUTING_TASK,
      pid: 1111,
      processPid: 2222,
      iteration: 1,
    });

    const agent = footer.agents.get('worker');
    assert.strictEqual(agent.processPid, 2222);
    assert.strictEqual(agent.pid, 2222);
  });
});

describe('StatusFooter runMode badge', () => {
  it('shows the armed run mode in the header line', () => {
    const footer = new StatusFooter({ enabled: false });
    footer.setCluster('cluster-test');
    footer.setRunMode('ship');
    const line = footer.stripAnsi(footer.buildHeaderLine(80));
    assert.ok(line.includes('[ship]'), `expected header to include [ship], got: ${line}`);
  });

  it('omits the badge when no run mode is set', () => {
    const footer = new StatusFooter({ enabled: false });
    footer.setCluster('cluster-test');
    const line = footer.stripAnsi(footer.buildHeaderLine(80));
    assert.ok(!line.includes('['), `expected no badge, got: ${line}`);
  });
});
