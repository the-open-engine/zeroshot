const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(__dirname, '..', '..', 'lib', 'tui', 'services', 'agent-messages.js');

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const {
  agentMessageKey,
  createPendingAgentMessage,
} = require('../../lib/tui/services/agent-messages');

describe('TUI agent messages', function () {
  it('builds a stable key for cluster and agent', function () {
    assert.strictEqual(agentMessageKey('cluster-1', 'agent-a'), 'cluster-1:agent-a');
  });

  it('creates a pending message payload', function () {
    const message = createPendingAgentMessage({
      clusterId: 'cluster-1',
      agentId: 'agent-a',
      text: 'hello',
      now: 1700000000000,
      id: 'msg-1',
    });

    assert.deepStrictEqual(message, {
      id: 'msg-1',
      clusterId: 'cluster-1',
      agentId: 'agent-a',
      text: 'hello',
      createdAt: 1700000000000,
      status: 'pending',
    });
  });
});
