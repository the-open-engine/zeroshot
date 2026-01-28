const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(
  __dirname,
  '..',
  '..',
  'lib',
  'tui',
  'services',
  'guidance-delivery.js'
);

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const {
  sendAgentGuidance,
  sendClusterGuidance,
} = require('../../lib/tui/services/guidance-delivery');

describe('TUI guidance delivery service', function () {
  it('delegates agent guidance to orchestrator', async function () {
    let captured;
    const orchestrator = {
      sendGuidanceToAgent: (clusterId, agentId, text, options) => {
        captured = { clusterId, agentId, text, options };
        return Promise.resolve({
          status: 'injected',
          reason: null,
          method: 'pty',
          taskId: 'task-1',
        });
      },
    };
    const result = await sendAgentGuidance({
      clusterId: 'cluster-1',
      agentId: 'agent-1',
      text: 'hello',
      timeoutMs: 123,
      deps: {
        getOrchestrator: () => Promise.resolve(orchestrator),
      },
    });

    assert.deepStrictEqual(captured, {
      clusterId: 'cluster-1',
      agentId: 'agent-1',
      text: 'hello',
      options: { timeoutMs: 123 },
    });
    assert.strictEqual(result.status, 'injected');
    assert.strictEqual(result.taskId, 'task-1');
  });

  it('delegates cluster guidance to orchestrator', async function () {
    let captured;
    const orchestrator = {
      sendGuidanceToCluster: (clusterId, text, options) => {
        captured = { clusterId, text, options };
        return Promise.resolve({
          summary: { injected: 1, queued: 1, total: 2 },
          agents: {
            'agent-1': { status: 'injected', reason: null, method: 'pty' },
            'agent-2': { status: 'unsupported', reason: 'no pty', method: null },
          },
          timestamp: 123,
        });
      },
    };

    const result = await sendClusterGuidance({
      clusterId: 'cluster-2',
      text: 'cluster guidance',
      timeoutMs: 456,
      deps: {
        getOrchestrator: () => Promise.resolve(orchestrator),
      },
    });

    assert.deepStrictEqual(captured, {
      clusterId: 'cluster-2',
      text: 'cluster guidance',
      options: { timeoutMs: 456 },
    });
    assert.strictEqual(result.summary.total, 2);
    assert.strictEqual(result.agents['agent-1'].status, 'injected');
  });
});
