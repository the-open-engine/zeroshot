const assert = require('node:assert');

const Orchestrator = require('../../src/orchestrator');
const { generateGitPusherAgent } = require('../../src/agents/git-pusher-template');
const { getConfig } = require('../../src/config-router');

function createPrCluster() {
  const gitPusher = generateGitPusherAgent('github');
  return {
    id: 'cluster-pr',
    autoPr: true,
    agents: [{ id: 'git-pusher', config: gitPusher }],
    config: {
      agents: [gitPusher],
    },
    messageBus: {
      publish: () => {},
    },
  };
}

function addPrRepairTrigger(agent) {
  if (agent.role !== 'implementation') {
    return agent;
  }
  const hasRepairTrigger = agent.triggers?.some((trigger) => trigger.topic === 'PUSH_BLOCKED');
  if (hasRepairTrigger) {
    return agent;
  }
  return {
    ...agent,
    triggers: [...(agent.triggers || []), { topic: 'PUSH_BLOCKED', action: 'execute_task' }],
  };
}

describe('pr-mode cluster validation', function () {
  it('accepts trivial task topology in autoPr mode', function () {
    const orchestrator = new Orchestrator({ quiet: true, skipLoad: true });
    const cluster = createPrCluster();
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

  it('accepts critical staged validation when git-pusher can publish PUSH_BLOCKED', function () {
    const orchestrator = new Orchestrator({ quiet: true, skipLoad: true });
    const cluster = createPrCluster();
    const gitPusher = cluster.config.agents[0];

    const fullWorkflowAgents = orchestrator
      ._buildProposedAgentConfigs(
        [],
        [
          {
            action: 'load_config',
            config: getConfig('CRITICAL', 'TASK', { autoPr: true }),
          },
        ]
      )
      .map(addPrRepairTrigger);

    const existingAgents = [gitPusher, ...fullWorkflowAgents];
    const operations = [
      {
        action: 'load_config',
        config: {
          base: 'quick-validation',
          params: { validator_level: 'level2', max_tokens: 150000, timeout: 0 },
        },
      },
      {
        action: 'publish',
        topic: 'IMPLEMENTATION_READY',
        content: {
          text: 'done',
          data: { completionStatus: { canValidate: true } },
        },
        metadata: { _republished: true },
      },
    ];
    const proposed = orchestrator._buildProposedAgentConfigs(existingAgents, operations);

    assert(proposed.some((agent) => agent.id === 'meta-coordinator'));
    assert(proposed.some((agent) => agent.id === 'consensus-coordinator'));
    assert(
      proposed
        .find((agent) => agent.id === 'worker')
        ?.triggers?.some((trigger) => trigger.topic === 'PUSH_BLOCKED'),
      'worker should keep the PR repair trigger that the runtime injected'
    );

    assert.doesNotThrow(() => {
      orchestrator._validateProposedConfig(cluster.id, cluster, proposed, operations);
    });
  });
});
