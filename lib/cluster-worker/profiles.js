'use strict';

const { resolveRunPlan } = require('../run-plan');
const { loadSettings } = require('../settings');
const { VALID_PROVIDERS, normalizeProviderName } = require('../provider-names');
const { deepFreeze } = require('./object-utils');

const DEFAULT_BOUNDS = Object.freeze({
  executionMs: 60 * 60 * 1000,
  shutdownMs: 30 * 1000,
  frameBytes: 64 * 1024,
});

const DEFAULT_ISOLATION_PROFILES = Object.freeze({
  'isolation.worktree@1': Object.freeze({ worktree: true }),
  'isolation.docker@1': Object.freeze({ docker: true }),
  'isolation.pr@1': Object.freeze({ pr: true }),
  'isolation.ship@1': Object.freeze({ ship: true }),
});

const DEFAULT_PROVIDER_PROFILES = Object.freeze({
  'provider.default@1': Object.freeze({
    configName: 'conductor-bootstrap',
    providerOverride: null,
    settings: Object.freeze({}),
  }),
});

const HOSTED_PROFILE = 'provider.hosted@1';
const HOSTED_PR_PROFILE = 'provider.hosted-pr@1';

function providerProfilesFromEnvironment(environment = process.env, settings) {
  const provider = normalizeProviderName(environment.ZEROSHOT_HOSTED_PROVIDER);
  if (!provider) {
    return DEFAULT_PROVIDER_PROFILES;
  }
  if (!VALID_PROVIDERS.includes(provider)) {
    throw new Error(`Unsupported hosted provider: ${provider}`);
  }
  const resolvedSettings = settings === undefined ? loadSettings({ silent: true }) : settings;
  const modelOverride = environment.ZEROSHOT_HOSTED_MODEL || undefined;
  const hostedSettings = { ...resolvedSettings, templateParams: { task_type: 'TASK' } };
  return deepFreeze({
    ...DEFAULT_PROVIDER_PROFILES,
    [HOSTED_PROFILE]: {
      configName: 'base-templates/single-worker',
      providerOverride: provider,
      modelOverride,
      settings: hostedSettings,
    },
    [HOSTED_PR_PROFILE]: {
      configName: 'base-templates/worker-validator',
      providerOverride: provider,
      modelOverride,
      settings: hostedSettings,
    },
  });
}

function own(map, key) {
  return Object.prototype.hasOwnProperty.call(map, key) ? map[key] : null;
}

function validateBounds(bounds) {
  for (const name of ['executionMs', 'shutdownMs', 'frameBytes']) {
    if (!Number.isSafeInteger(bounds[name]) || bounds[name] <= 0) {
      throw new Error(`Deployment bound ${name} must be a positive safe integer`);
    }
  }
}

function createDeploymentProfileRegistry(options = {}) {
  const isolationProfiles = options.isolationProfiles || DEFAULT_ISOLATION_PROFILES;
  const providerProfiles = options.providerProfiles || providerProfilesFromEnvironment();
  const defaultBounds = { ...DEFAULT_BOUNDS, ...(options.bounds || {}) };
  validateBounds(defaultBounds);
  const bounds = deepFreeze(defaultBounds);

  return Object.freeze({
    bounds,
    resolve(isolationHandle, providerHandle) {
      const deployment = own(isolationProfiles, isolationHandle);
      if (!deployment) throw new Error(`Unknown isolation profile: ${String(isolationHandle)}`);
      const provider = own(providerProfiles, providerHandle);
      if (!provider) throw new Error(`Unknown provider profile: ${String(providerHandle)}`);

      const plan = resolveRunPlan(deployment);
      if (plan.isolation !== 'worktree' && plan.isolation !== 'docker') {
        throw new Error(`Isolation profile ${isolationHandle} resolves to non-isolated execution`);
      }
      const resolvedBounds = {
        ...defaultBounds,
        ...(deployment.bounds || {}),
        ...(provider.bounds || {}),
      };
      validateBounds(resolvedBounds);
      return deepFreeze({
        isolationProfile: isolationHandle,
        providerProfile: providerHandle,
        plan,
        deployment: { ...deployment },
        provider: { ...provider },
        bounds: resolvedBounds,
      });
    },
  });
}

module.exports = {
  DEFAULT_BOUNDS,
  DEFAULT_ISOLATION_PROFILES,
  DEFAULT_PROVIDER_PROFILES,
  HOSTED_PROFILE,
  HOSTED_PR_PROFILE,
  createDeploymentProfileRegistry,
  providerProfilesFromEnvironment,
};
