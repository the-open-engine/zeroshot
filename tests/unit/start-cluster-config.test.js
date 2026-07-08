const assert = require('assert');

const { loadClusterConfig, buildStartOptions } = require('../../lib/start-cluster');

function createOrchestrator(config) {
  const calls = { loadConfig: [] };
  return {
    calls,
    loadConfig(configPath) {
      calls.loadConfig.push(configPath);
      return config;
    },
  };
}

describe('start-cluster config loading', function () {
  it('resolves parameterized config files with defaults before agents are created', function () {
    const orchestrator = createOrchestrator({
      name: 'Parameterized',
      params: {
        planner_level: { type: 'string', default: 'level3' },
        task_type: { type: 'string', default: 'TASK' },
        timeout: { type: 'number', default: 0 },
      },
      agents: [
        {
          id: 'planner',
          role: 'planning',
          modelLevel: '{{planner_level}}',
          timeout: '{{timeout}}',
          prompt: {
            system: 'Plan a {{task_type}}',
          },
        },
      ],
    });

    const config = loadClusterConfig(orchestrator, '/tmp/parameterized.json', {
      defaultProvider: 'claude',
      providerSettings: {},
    });

    assert.strictEqual(config.params, undefined);
    assert.strictEqual(config.defaultProvider, 'claude');
    assert.strictEqual(config.agents[0].modelLevel, 'level3');
    assert.strictEqual(config.agents[0].timeout, '0');
    assert.strictEqual(config.agents[0].prompt.system, 'Plan a TASK');
  });
});

describe('buildStartOptions() runMode', function () {
  it('derives runMode "ship" from pre-transform mergedOptions', function () {
    const result = buildStartOptions({ clusterId: 'c1', options: { ship: true } });
    assert.strictEqual(result.runMode, 'ship');
  });

  it('derives runMode "pr+docker" even though transformed output has no .pr/.docker keys', function () {
    const result = buildStartOptions({ clusterId: 'c1', options: { pr: true, docker: true } });
    assert.strictEqual(result.runMode, 'pr+docker');
    assert.strictEqual(result.pr, undefined);
    assert.strictEqual(result.docker, undefined);
  });

  it('returns null runMode when no isolation flags are set', function () {
    const result = buildStartOptions({ clusterId: 'c1', options: {} });
    assert.strictEqual(result.runMode, null);
  });
});
