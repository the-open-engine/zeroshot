/**
 * Tests for execute_system_command trigger action in agent-lifecycle.js
 */

const assert = require('assert');
const path = require('path');
const { executeTriggerAction } = require('../src/agent/agent-lifecycle');

function createMockAgent(overrides = {}) {
  const published = [];
  const logs = [];
  return {
    id: 'test-agent',
    role: 'orchestrator',
    state: 'idle',
    cwd: overrides.cwd || process.cwd(),
    cluster: { id: 'test-cluster-123' },
    _publish: (msg) => published.push(msg),
    _log: (msg) => logs.push(msg),
    published,
    logs,
    ...overrides,
  };
}

describe('execute_system_command trigger action', function () {
  it('should pipe message.content as JSON to stdin', async function () {
    const agent = createMockAgent();
    const content = { text: 'hello', data: { foo: 'bar' } };

    // Use a command that reads stdin and echoes it
    const trigger = {
      action: 'execute_system_command',
      config: { command: 'cat' },
    };
    const message = { content, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    // cat echoes stdin to stdout, which gets logged
    assert.ok(
      agent.logs.some((l) => l.includes(JSON.stringify(content))),
      `Expected logs to contain stringified content. Logs: ${agent.logs.join('\n')}`
    );
  });

  it('should set ZEROSHOT_ROOT and CLUSTER_ID env vars', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'echo "ROOT=$ZEROSHOT_ROOT CLUSTER=$CLUSTER_ID"',
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    const expectedRoot = path.join(__dirname, '..');
    const outputLog = agent.logs.find((l) => l.includes('System command output:'));
    assert.ok(outputLog, `Expected output log. Logs: ${agent.logs.join('\n')}`);
    assert.ok(
      outputLog.includes(`ROOT=${expectedRoot}`),
      `Expected ZEROSHOT_ROOT in output: ${outputLog}`
    );
    assert.ok(
      outputLog.includes('CLUSTER=test-cluster-123'),
      `Expected CLUSTER_ID in output: ${outputLog}`
    );
  });

  it('should publish CLUSTER_COMPLETE when stopClusterAfter is true', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'echo ok',
        stopClusterAfter: true,
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.state, 'completed');
    assert.strictEqual(agent.published.length, 1);
    assert.strictEqual(agent.published[0].topic, 'CLUSTER_COMPLETE');
    assert.strictEqual(agent.published[0].content.data.reason, 'system_command_complete');
  });

  it('should set agent state to idle when stopClusterAfter is absent', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: { command: 'echo ok' },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.state, 'idle');
    assert.strictEqual(agent.published.length, 0);
  });

  it('should set agent state to idle when stopClusterAfter is false', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: { command: 'echo ok', stopClusterAfter: false },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.state, 'idle');
    assert.strictEqual(agent.published.length, 0);
  });

  it('should throw on missing config.command', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {},
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await assert.rejects(
      () => executeTriggerAction(agent, trigger, message),
      (err) => {
        assert.ok(err.message.includes('execute_system_command requires config.command'));
        return true;
      }
    );
  });

  it('should throw on missing config entirely', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await assert.rejects(
      () => executeTriggerAction(agent, trigger, message),
      (err) => {
        assert.ok(err.message.includes('execute_system_command requires config.command'));
        return true;
      }
    );
  });

  it('should throw on command failure (non-zero exit)', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: { command: 'exit 1' },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await assert.rejects(
      () => executeTriggerAction(agent, trigger, message),
      (err) => {
        assert.ok(err.status === 1 || err.message.includes('exit'));
        return true;
      }
    );
  });

  it('should respect config.timeout', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'sleep 10',
        timeout: 500,
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    const start = Date.now();
    await assert.rejects(
      () => executeTriggerAction(agent, trigger, message),
      () => {
        const elapsed = Date.now() - start;
        // Should fail quickly (within 2s) due to timeout, not wait 10s
        assert.ok(elapsed < 5000, `Took too long: ${elapsed}ms`);
        return true;
      }
    );
  });

  it('should handle empty stdin when message has no content', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: { command: 'echo ok' },
    };
    const message = { topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);
    assert.strictEqual(agent.state, 'idle');
  });

  it('should use agent.cwd as working directory', async function () {
    const agent = createMockAgent({ cwd: '/tmp' });
    const trigger = {
      action: 'execute_system_command',
      config: { command: 'pwd' },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    const outputLog = agent.logs.find((l) => l.includes('System command output:'));
    assert.ok(outputLog, `Expected output log. Logs: ${agent.logs.join('\n')}`);
    assert.ok(outputLog.includes('/tmp'), `Expected /tmp in output: ${outputLog}`);
  });
});
