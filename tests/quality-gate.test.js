/**
 * Tests for quality gate feature:
 * - execute_system_command onSuccess/onFailure behavior
 * - quality-gate-runner.js script
 */

const assert = require('assert');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { execSync } = require('child_process');
const { executeTriggerAction } = require('../src/agent/agent-lifecycle');

function createMockAgent(overrides = {}) {
  const published = [];
  const logs = [];
  return {
    id: 'quality-gate',
    role: 'quality-gate',
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

describe('execute_system_command onSuccess/onFailure', function () {
  it('should publish to onSuccess.topic when command succeeds', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'echo "all tests passed"',
        onSuccess: { topic: 'QUALITY_GATE_PASSED' },
        onFailure: { topic: 'QUALITY_GATE_FAILED' },
      },
    };
    const message = { content: { text: 'test' }, topic: 'IMPLEMENTATION_READY' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.state, 'idle');
    assert.strictEqual(agent.published.length, 1);
    assert.strictEqual(agent.published[0].topic, 'QUALITY_GATE_PASSED');
    assert.strictEqual(agent.published[0].content.data.exitCode, 0);
    assert.ok(agent.published[0].content.data.output.includes('all tests passed'));
  });

  it('should publish to onFailure.topic when command fails', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'echo "lint error" >&2; exit 1',
        onSuccess: { topic: 'QUALITY_GATE_PASSED' },
        onFailure: { topic: 'QUALITY_GATE_FAILED' },
      },
    };
    const message = { content: { text: 'test' }, topic: 'IMPLEMENTATION_READY' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.state, 'idle', 'Agent should be idle, not failed');
    assert.strictEqual(agent.published.length, 1);
    assert.strictEqual(agent.published[0].topic, 'QUALITY_GATE_FAILED');
    assert.ok(agent.published[0].content.data.exitCode > 0);
    assert.ok(agent.published[0].content.data.stderr.includes('lint error'));
  });

  it('should set state to idle (not failed) when onFailure is configured', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'exit 2',
        onFailure: { topic: 'QUALITY_GATE_FAILED' },
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.state, 'idle');
    assert.notStrictEqual(agent.state, 'failed');
  });

  it('should preserve CLUSTER_FAILED behavior when onFailure is absent', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: { command: 'exit 1' },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.state, 'failed');
    assert.strictEqual(agent.published.length, 1);
    assert.strictEqual(agent.published[0].topic, 'CLUSTER_FAILED');
    assert.strictEqual(agent.published[0].content.data.reason, 'system_command_error');
  });

  it('should truncate stdout/stderr at 5000 chars in onFailure', async function () {
    const agent = createMockAgent();
    // Generate output longer than 5000 chars
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'python3 -c "print(\'x\' * 10000)" >&2; exit 1',
        onFailure: { topic: 'QUALITY_GATE_FAILED' },
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    const data = agent.published[0].content.data;
    assert.ok(data.stderr.length > 0, 'stderr should not be empty');
    assert.ok(data.stderr.length <= 5100, `stderr too long: ${data.stderr.length}`);
    assert.ok(data.stderr.includes('...(truncated)...'));
  });

  it('should truncate output at 5000 chars in onSuccess', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'python3 -c "print(\'x\' * 10000)"',
        onSuccess: { topic: 'QUALITY_GATE_PASSED' },
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    const data = agent.published[0].content.data;
    assert.ok(data.output.length > 0, 'output should not be empty');
    assert.ok(data.output.length <= 5100, `output too long: ${data.output.length}`);
    assert.ok(data.output.includes('...(truncated)...'));
  });

  it('should include command in onFailure message data', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'echo fail; exit 1',
        onFailure: { topic: 'QUALITY_GATE_FAILED' },
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.published[0].content.data.command, 'echo fail; exit 1');
  });

  it('should work with onSuccess only (no onFailure)', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'echo ok',
        onSuccess: { topic: 'CUSTOM_PASSED' },
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.state, 'idle');
    assert.strictEqual(agent.published[0].topic, 'CUSTOM_PASSED');
  });

  it('should work with onFailure only (no onSuccess)', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'echo ok',
        onFailure: { topic: 'CUSTOM_FAILED' },
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    // Command succeeded, no onSuccess configured, falls through to default behavior
    assert.strictEqual(agent.state, 'idle');
    assert.strictEqual(agent.published.length, 0);
  });
});

describe('quality-gate-runner.js', function () {
  const runnerPath = path.join(__dirname, '..', 'scripts', 'quality-gate-runner.js');
  let tmpDir;

  beforeEach(function () {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'qg-test-'));
  });

  afterEach(function () {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it('should auto-pass when .zeroshot-quality is missing', function () {
    const output = execSync(`node ${runnerPath}`, {
      cwd: tmpDir,
      encoding: 'utf-8',
    });

    const result = JSON.parse(output.trim());
    assert.strictEqual(result.exitCode, 0);
    assert.strictEqual(result.command, null);
    assert.ok(result.stdout.includes('auto-passed'));
  });

  it('should auto-pass when .zeroshot-quality is empty', function () {
    fs.writeFileSync(path.join(tmpDir, '.zeroshot-quality'), '');
    const output = execSync(`node ${runnerPath}`, {
      cwd: tmpDir,
      encoding: 'utf-8',
    });

    const result = JSON.parse(output.trim());
    assert.strictEqual(result.exitCode, 0);
    assert.strictEqual(result.command, null);
    assert.ok(result.stdout.includes('empty'));
  });

  it('should run command and exit 0 on success', function () {
    fs.writeFileSync(path.join(tmpDir, '.zeroshot-quality'), 'echo "checks passed"');
    const output = execSync(`node ${runnerPath}`, {
      cwd: tmpDir,
      encoding: 'utf-8',
    });

    const result = JSON.parse(output.trim());
    assert.strictEqual(result.exitCode, 0);
    assert.strictEqual(result.command, 'echo "checks passed"');
    assert.ok(result.stdout.includes('checks passed'));
  });

  it('should run command and exit non-zero on failure', function () {
    fs.writeFileSync(path.join(tmpDir, '.zeroshot-quality'), 'echo "test failed" >&2; exit 1');

    let output;
    try {
      execSync(`node ${runnerPath}`, {
        cwd: tmpDir,
        encoding: 'utf-8',
      });
      assert.fail('Should have thrown');
    } catch (error) {
      assert.strictEqual(error.status, 1);
      output = error.stdout;
    }

    const result = JSON.parse(output.trim());
    assert.strictEqual(result.exitCode, 1);
    assert.ok(result.stderr.includes('test failed'));
  });

  it('should capture both stdout and stderr', function () {
    fs.writeFileSync(
      path.join(tmpDir, '.zeroshot-quality'),
      'echo "out-message"; echo "err-message" >&2; exit 1'
    );

    let output;
    try {
      execSync(`node ${runnerPath}`, {
        cwd: tmpDir,
        encoding: 'utf-8',
      });
      assert.fail('Should have thrown');
    } catch (error) {
      output = error.stdout;
    }

    const result = JSON.parse(output.trim());
    assert.ok(result.stdout.includes('out-message'));
    assert.ok(result.stderr.includes('err-message'));
  });
});

describe('config-validator: onSuccess/onFailure validation', function () {
  const { validateBasicStructure } = require('../src/config-validator');

  it('should reject empty onSuccess.topic', function () {
    const config = {
      agents: [
        {
          id: 'qg',
          role: 'quality-gate',
          triggers: [
            {
              topic: 'IMPLEMENTATION_READY',
              action: 'execute_system_command',
              config: {
                command: 'echo ok',
                onSuccess: { topic: '' },
              },
            },
          ],
        },
      ],
    };

    const result = validateBasicStructure(config);
    assert.ok(
      result.errors.some((e) => e.includes('onSuccess.topic must be a non-empty string')),
      `Expected onSuccess validation error. Errors: ${result.errors.join(', ')}`
    );
  });

  it('should reject missing onFailure.topic', function () {
    const config = {
      agents: [
        {
          id: 'qg',
          role: 'quality-gate',
          triggers: [
            {
              topic: 'IMPLEMENTATION_READY',
              action: 'execute_system_command',
              config: {
                command: 'echo ok',
                onFailure: {},
              },
            },
          ],
        },
      ],
    };

    const result = validateBasicStructure(config);
    assert.ok(
      result.errors.some((e) => e.includes('onFailure.topic must be a non-empty string')),
      `Expected onFailure validation error. Errors: ${result.errors.join(', ')}`
    );
  });

  it('should accept valid onSuccess/onFailure config', function () {
    const config = {
      agents: [
        {
          id: 'qg',
          role: 'quality-gate',
          triggers: [
            {
              topic: 'IMPLEMENTATION_READY',
              action: 'execute_system_command',
              config: {
                command: 'echo ok',
                onSuccess: { topic: 'QUALITY_GATE_PASSED' },
                onFailure: { topic: 'QUALITY_GATE_FAILED' },
              },
            },
          ],
        },
      ],
    };

    const result = validateBasicStructure(config);
    const relevantErrors = result.errors.filter(
      (e) => e.includes('onSuccess') || e.includes('onFailure')
    );
    assert.strictEqual(
      relevantErrors.length,
      0,
      `Unexpected validation errors: ${relevantErrors.join(', ')}`
    );
  });

  it('should reject execute_system_command with missing config', function () {
    const config = {
      agents: [
        {
          id: 'qg',
          role: 'quality-gate',
          triggers: [
            {
              topic: 'IMPLEMENTATION_READY',
              action: 'execute_system_command',
            },
          ],
        },
      ],
    };

    const result = validateBasicStructure(config);
    assert.ok(
      result.errors.some((e) => e.includes('config is required for execute_system_command')),
      `Expected missing config error. Errors: ${result.errors.join(', ')}`
    );
  });

  it('should reject execute_system_command with missing config.command', function () {
    const config = {
      agents: [
        {
          id: 'qg',
          role: 'quality-gate',
          triggers: [
            {
              topic: 'IMPLEMENTATION_READY',
              action: 'execute_system_command',
              config: {
                onSuccess: { topic: 'PASSED' },
              },
            },
          ],
        },
      ],
    };

    const result = validateBasicStructure(config);
    assert.ok(
      result.errors.some((e) =>
        e.includes('config.command is required for execute_system_command')
      ),
      `Expected missing command error. Errors: ${result.errors.join(', ')}`
    );
  });
});

describe('execute_system_command timeout and error handling', function () {
  it('should publish to onFailure.topic when command times out', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'sleep 10',
        timeout: 500,
        onFailure: { topic: 'QUALITY_GATE_FAILED' },
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.state, 'idle');
    assert.strictEqual(agent.published.length, 1);
    assert.strictEqual(agent.published[0].topic, 'QUALITY_GATE_FAILED');
    assert.ok(agent.published[0].content.data.exitCode > 0);
  });

  it('should publish to onFailure.topic when command not found', async function () {
    const agent = createMockAgent();
    const trigger = {
      action: 'execute_system_command',
      config: {
        command: 'nonexistent_command_xyz_12345',
        onFailure: { topic: 'QUALITY_GATE_FAILED' },
      },
    };
    const message = { content: { text: 'test' }, topic: 'TEST' };

    await executeTriggerAction(agent, trigger, message);

    assert.strictEqual(agent.state, 'idle');
    assert.strictEqual(agent.published.length, 1);
    assert.strictEqual(agent.published[0].topic, 'QUALITY_GATE_FAILED');
    assert.ok(agent.published[0].content.data.exitCode > 0);
  });
});

describe('quality-gate-runner.js error handling', function () {
  const runnerPath = path.join(__dirname, '..', 'scripts', 'quality-gate-runner.js');
  let tmpDir;

  beforeEach(function () {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'qg-test-'));
  });

  afterEach(function () {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it('should report non-zero exit when command not found', function () {
    fs.writeFileSync(path.join(tmpDir, '.zeroshot-quality'), 'nonexistent_command_xyz_12345');

    let output;
    try {
      execSync(`node ${runnerPath}`, {
        cwd: tmpDir,
        encoding: 'utf-8',
      });
      assert.fail('Should have thrown');
    } catch (error) {
      assert.ok(error.status > 0, `Expected non-zero exit, got ${error.status}`);
      output = error.stdout;
    }

    const result = JSON.parse(output.trim());
    assert.ok(result.exitCode > 0);
    assert.ok(result.stderr.length > 0, 'stderr should contain error info');
  });
});
