const assert = require('node:assert');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const sinon = require('sinon');

const Ledger = require('../../src/ledger');
const MessageBus = require('../../src/message-bus');
const Orchestrator = require('../../src/orchestrator');

describe('Orchestrator critical agent error handling', function () {
  this.timeout(10_000);

  let tempDir;
  let ledger;
  let messageBus;
  let orchestrator;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-orchestrator-agent-error-'));
    ledger = new Ledger(path.join(tempDir, 'test.db'));
    messageBus = new MessageBus(ledger);

    orchestrator = new Orchestrator({ quiet: true, skipLoad: true, storageDir: tempDir });
    sinon.stub(orchestrator, '_saveClusters').resolves();
  });

  afterEach(() => {
    sinon.restore();
    if (ledger) ledger.close();
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('stops cluster when coordinator fails after retries', async () => {
    const stopSpy = sinon.stub(orchestrator, 'stop').resolves();
    orchestrator._registerAgentErrorHandler(messageBus, 'c1');

    messageBus.publish({
      cluster_id: 'c1',
      topic: 'AGENT_ERROR',
      sender: 'consensus-coordinator',
      content: { data: { role: 'coordinator', attempts: 3, error: 'boom' } },
    });

    await new Promise((r) => setTimeout(r, 10));
    assert.equal(stopSpy.calledOnce, true);
    assert.equal(stopSpy.firstCall.args[0], 'c1');
  });

  it('stops cluster immediately when hookFailure is true (even with attempts=1)', async () => {
    const stopSpy = sinon.stub(orchestrator, 'stop').resolves();
    orchestrator._registerAgentErrorHandler(messageBus, 'c2');

    messageBus.publish({
      cluster_id: 'c2',
      topic: 'AGENT_ERROR',
      sender: 'consensus-coordinator',
      content: {
        data: { role: 'coordinator', attempts: 1, hookFailure: true, error: 'hook died' },
      },
    });

    await new Promise((r) => setTimeout(r, 10));
    assert.equal(stopSpy.calledOnce, true);
    assert.equal(stopSpy.firstCall.args[0], 'c2');
  });

  it('does not stop cluster for validator errors by default', async () => {
    const stopSpy = sinon.stub(orchestrator, 'stop').resolves();
    orchestrator._registerAgentErrorHandler(messageBus, 'c3');

    messageBus.publish({
      cluster_id: 'c3',
      topic: 'AGENT_ERROR',
      sender: 'validator-1',
      content: { data: { role: 'validator', attempts: 3, error: 'nope' } },
    });

    await new Promise((r) => setTimeout(r, 10));
    assert.equal(stopSpy.called, false);
  });
});
