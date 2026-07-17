'use strict';

const assert = require('assert');
const { createCurrentEngineAdapter } = require('../../lib/cluster-worker/engine-adapter');
const { createDeploymentProfileRegistry } = require('../../lib/cluster-worker/profiles');
const { validateAgentConfig } = require('../../src/agent/agent-config');

const EXPECTED_DELIVERY = {
  'isolation.worktree@1': { autoMerge: false, autoPr: false, ship: false },
  'isolation.pr@1': { autoMerge: false, autoPr: true, ship: false },
  'isolation.ship@1': { autoMerge: true, autoPr: true, ship: true },
};

function createInlineRegistry() {
  return createDeploymentProfileRegistry({
    providerProfiles: {
      'provider.inline@1': {
        config: { agents: [{ id: 'worker', role: 'implementation' }] },
        providerOverride: 'codex',
        settings: { defaultProvider: 'claude', providerSettings: {} },
      },
    },
  });
}

function assertPreparedConfig(config, profile, options, expectedDelivery) {
  assert.notStrictEqual(config, profile.provider.config);
  assert.strictEqual(Object.isFrozen(config), false);
  assert.strictEqual(Object.isFrozen(config.agents[0]), false);
  assert.strictEqual(config.forceProvider, 'codex');
  assert.strictEqual(config.defaultProvider, 'codex');
  assert.strictEqual(config.defaultLevel, config.forceLevel);
  assert.deepStrictEqual(
    { autoMerge: options.autoMerge, autoPr: options.autoPr, ship: options.ship },
    expectedDelivery
  );

  // Match Orchestrator: allocated worktree cwd first, mutable AgentWrapper defaults second.
  config.agents[0].cwd = `/allocated/${profile.plan.delivery}`;
  const normalized = validateAgentConfig(config.agents[0]);
  assert.strictEqual(normalized.cwd, `/allocated/${profile.plan.delivery}`);
  assert.strictEqual(normalized.timeout, 0);
}

function createHarness(profile, expectedDelivery) {
  const messageBus = { subscribe: () => () => {}, getAll: () => [] };
  let executionConfig;
  const orchestrator = {
    start(config, _input, options) {
      executionConfig = config;
      assertPreparedConfig(config, profile, options, expectedDelivery);
      return { id: `cluster-${profile.plan.delivery}`, messageBus };
    },
    close() {},
  };
  return {
    adapter: createCurrentEngineAdapter({ orchestrator }),
    executionConfig: () => executionConfig,
  };
}

describe('legacy cluster worker inline configs', () => {
  it('prepares mutable execution-local configs for worktree, PR, and ship plans', async () => {
    const registry = createInlineRegistry();
    for (const [isolationProfile, expectedDelivery] of Object.entries(EXPECTED_DELIVERY)) {
      const profile = registry.resolve(isolationProfile, 'provider.inline@1');
      const harness = createHarness(profile, expectedDelivery);
      await harness.adapter.start({
        request: { source: 'prompt', prompt: 'task' },
        profile,
        artifactManifest: { artifacts: [] },
        clusterId: `cluster-${profile.plan.delivery}`,
        onEvent() {},
      });

      assert.ok(harness.executionConfig());
      assert.strictEqual(profile.provider.config.agents[0].cwd, undefined);
      assert.strictEqual(profile.provider.config.agents[0].timeout, undefined);
      harness.adapter.close();
    }
  });
});
