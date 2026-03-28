const assert = require('node:assert');

const Orchestrator = require('../../src/orchestrator');
const { getConfig } = require('../../src/config-router');

describe('pr-mode cluster validation', function () {
  it('accepts trivial task topology in autoPr mode', function () {
    const orchestrator = new Orchestrator({ quiet: true, skipLoad: true });
    const cluster = {
      id: 'cluster-pr',
      autoPr: true,
      agents: [{ id: 'git-pusher', config: { id: 'git-pusher' } }],
      config: {
        agents: [{ id: 'git-pusher', role: 'completion-detector' }],
      },
      messageBus: {
        publish: () => {},
      },
    };
    const operations = [
      {
        action: 'load_config',
        config: getConfig('TRIVIAL', 'TASK', { autoPr: true }),
      },
    ];
    const proposed = orchestrator._buildProposedAgentConfigs([], operations);

    assert(proposed.some((agent) => agent.id === 'validator'));
    assert.doesNotThrow(() => {
      orchestrator._validateProposedConfig(cluster.id, cluster, proposed, operations);
    });
  });
});
