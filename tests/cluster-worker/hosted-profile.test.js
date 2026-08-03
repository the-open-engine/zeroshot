'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const { describe, it } = require('mocha');
const {
  HOSTED_PR_PROFILE,
  HOSTED_PROFILE,
  createDeploymentProfileRegistry,
  providerProfilesFromEnvironment,
} = require('../../lib/cluster-worker/profiles');
const { prepareClusterConfig, resolveConfigPath } = require('../../lib/start-cluster');

function settings(provider) {
  return {
    defaultProvider: provider,
    providerSettings: {
      [provider]: { defaultLevel: 'level2' },
    },
  };
}

describe('generic hosted deployment profiles', function () {
  it('is absent unless the capsule host supplies a runtime provider', function () {
    const profiles = providerProfilesFromEnvironment({}, settings('gemini'));
    assert.equal(profiles[HOSTED_PROFILE], undefined);
    assert.equal(profiles[HOSTED_PR_PROFILE], undefined);
  });

  it('uses reviewed delivery for any locally supported provider', function () {
    const providerProfiles = providerProfilesFromEnvironment(
      {
        ZEROSHOT_HOSTED_PROVIDER: 'gemini',
        ZEROSHOT_HOSTED_MODEL: 'gemini-2.5-pro',
      },
      settings('gemini')
    );
    const registry = createDeploymentProfileRegistry({ providerProfiles });
    const profile = registry.resolve('isolation.pr@1', HOSTED_PR_PROFILE);

    assert.equal(profile.plan.delivery, 'pr');
    assert.equal(profile.provider.configName, 'base-templates/worker-validator');
    assert.equal(profile.provider.providerOverride, 'gemini');
    assert.equal(profile.provider.modelOverride, 'gemini-2.5-pro');
    const config = prepareClusterConfig(
      JSON.parse(fs.readFileSync(resolveConfigPath(profile.provider.configName), 'utf8')),
      profile.provider.settings,
      profile.provider.providerOverride
    );
    assert.deepEqual(
      config.agents.map(({ id }) => id),
      ['worker', 'validator']
    );
  });

  it('carries the uploaded settings without interpreting endpoint or credential fields', function () {
    const uploadedSettings = {
      defaultProvider: 'gateway',
      providerSettings: {
        gateway: {
          defaultLevel: 'level2',
          protocol: 'openai',
          baseUrl: 'https://models.example/v1',
          apiKey: 'runtime-test-secret',
          model: 'private-model',
          toolPolicy: { roots: ['/workspace/repository'], commands: ['*'] },
        },
      },
    };
    const providerProfiles = providerProfilesFromEnvironment(
      { ZEROSHOT_HOSTED_PROVIDER: 'gateway' },
      uploadedSettings
    );
    const registry = createDeploymentProfileRegistry({ providerProfiles });
    const profile = registry.resolve('isolation.worktree@1', HOSTED_PROFILE);

    assert.equal(profile.provider.providerOverride, 'gateway');
    assert.equal(profile.provider.modelOverride, undefined);
    assert.equal(
      profile.provider.settings.providerSettings.gateway.baseUrl,
      'https://models.example/v1'
    );
    assert.equal(profile.provider.settings.providerSettings.gateway.apiKey, 'runtime-test-secret');
  });

  it('normalizes aliases and fails closed only outside the local provider registry', function () {
    const aliased = providerProfilesFromEnvironment(
      { ZEROSHOT_HOSTED_PROVIDER: 'anthropic' },
      settings('claude')
    );
    assert.equal(aliased[HOSTED_PROFILE].providerOverride, 'claude');
    assert.throws(
      () =>
        providerProfilesFromEnvironment(
          { ZEROSHOT_HOSTED_PROVIDER: 'future-harness' },
          settings('claude')
        ),
      /Unsupported hosted provider/
    );
  });
});
