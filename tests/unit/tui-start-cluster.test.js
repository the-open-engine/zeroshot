const assert = require('assert');

const { startClusterFromText, startClusterFromIssue } = require('../../lib/start-cluster');

function createOrchestrator() {
  const calls = { loadConfig: [], start: [] };
  return {
    calls,
    loadConfig(configPath) {
      calls.loadConfig.push(configPath);
      return { agents: [] };
    },
    start(config, input, options) {
      calls.start.push({ config, input, options });
      return { id: 'cluster-test' };
    },
  };
}

describe('TUI start cluster helper', function () {
  const originalCwd = process.env.ZEROSHOT_CWD;

  afterEach(function () {
    if (originalCwd === undefined) {
      delete process.env.ZEROSHOT_CWD;
    } else {
      process.env.ZEROSHOT_CWD = originalCwd;
    }
  });

  it('startClusterFromText builds text input and forwards providerOverride', async function () {
    process.env.ZEROSHOT_CWD = '/tmp';
    const orchestrator = createOrchestrator();
    const settings = { defaultProvider: 'claude', providerSettings: {} };
    const configPath = '/tmp/config.json';

    const result = await startClusterFromText({
      orchestrator,
      text: 'Launch cluster',
      configPath,
      settings,
      providerOverride: 'codex',
      modelOverride: 'gpt-4o',
      forceProvider: 'github',
      clusterId: 'cluster-123',
      options: { docker: false, worktree: false, pr: false, mounts: false },
    });

    assert.strictEqual(orchestrator.calls.loadConfig[0], configPath);
    assert.strictEqual(orchestrator.calls.start.length, 1);
    const call = orchestrator.calls.start[0];
    assert.deepStrictEqual(call.input, { text: 'Launch cluster' });
    assert.strictEqual(call.options.providerOverride, 'codex');
    assert.strictEqual(call.options.clusterId, 'cluster-123');
    assert.strictEqual(call.options.noMounts, true);
    assert.strictEqual(result.id, 'cluster-test');
  });

  it('startClusterFromIssue builds issue input and forwards providerOverride', async function () {
    process.env.ZEROSHOT_CWD = '/tmp';
    const orchestrator = createOrchestrator();
    const settings = { defaultProvider: 'claude', providerSettings: {} };
    const configPath = '/tmp/config.json';

    await startClusterFromIssue({
      orchestrator,
      issue: '123',
      configPath,
      settings,
      providerOverride: 'codex',
      clusterId: 'cluster-456',
      options: { docker: false, worktree: false, pr: false },
    });

    assert.strictEqual(orchestrator.calls.loadConfig[0], configPath);
    assert.strictEqual(orchestrator.calls.start.length, 1);
    const call = orchestrator.calls.start[0];
    assert.deepStrictEqual(call.input, { issue: '123' });
    assert.strictEqual(call.options.providerOverride, 'codex');
    assert.strictEqual(call.options.clusterId, 'cluster-456');
  });
});
