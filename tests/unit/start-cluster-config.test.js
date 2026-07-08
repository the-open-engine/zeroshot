const assert = require('assert');

const { loadClusterConfig } = require('../../lib/start-cluster');

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
