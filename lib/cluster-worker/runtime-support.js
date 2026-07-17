'use strict';

const { randomUUID } = require('crypto');
const { validateArtifactRef, validateWorkerOutcome } = require('./contracts');
const { createCurrentEngineAdapter } = require('./engine-adapter');
const { cloneJson, deepFreeze } = require('./object-utils');
const { createDeploymentProfileRegistry } = require('./profiles');
const { resolveRunPlan } = require('../run-plan');

const DEPENDENCY_KEYS = new Set([
  'profileRegistry',
  'artifactResolver',
  'artifactReceiptSink',
  'engineAdapter',
  'clock',
  'timers',
  'idFactory',
]);

function defaultArtifactResolver() {
  return Object.freeze({
    stage(artifacts) {
      for (const artifact of artifacts) validateArtifactRef(artifact);
      return deepFreeze({ artifacts: cloneJson(artifacts) });
    },
  });
}

async function stageArtifacts(resolver, artifacts, context) {
  if (artifacts.length === 0) return deepFreeze({ artifacts: [] });
  const stage = typeof resolver === 'function' ? resolver : resolver?.stage;
  if (typeof stage !== 'function') throw new Error('ArtifactResolver must expose stage()');
  const manifest = await stage.call(resolver, artifacts, context);
  if (!manifest || typeof manifest !== 'object' || Array.isArray(manifest)) {
    throw new Error('ArtifactResolver returned an invalid staged manifest');
  }
  return deepFreeze(manifest);
}

async function collectReceipts(sink, declared, context) {
  if (!sink) return [];
  const collect = typeof sink === 'function' ? sink : sink.collect || sink.write;
  if (typeof collect !== 'function') throw new Error('ArtifactReceiptSink must expose collect()');
  const receipts = await collect.call(sink, declared || [], context);
  if (!Array.isArray(receipts)) throw new Error('ArtifactReceiptSink must return an array');
  for (const receipt of receipts) validateArtifactRef(receipt);
  return receipts.map((receipt) => deepFreeze(cloneJson(receipt)));
}

function errorOutcome(code, reason) {
  const outcome = { status: 'error', code, reason };
  validateWorkerOutcome(outcome);
  return Object.freeze(outcome);
}

async function stopEngineWithinBound({ engineAdapter, timers, shutdownMs }) {
  let deadlineHandle = null;
  let effective = false;
  try {
    const engineStop = Promise.resolve()
      .then(() => engineAdapter.stop())
      .then(
        (stopResult) => stopResult?.effective !== false,
        () => false
      );
    const deadline = new Promise((resolve) => {
      deadlineHandle = timers.setTimeout(() => resolve(false), shutdownMs);
    });
    effective = await Promise.race([engineStop, deadline]);
  } catch {
    effective = false;
  }
  if (deadlineHandle !== null) {
    try {
      timers.clearTimeout(deadlineHandle);
    } catch {
      effective = false;
    }
  }
  return effective;
}

function resolveRuntimeDependencies(dependencies) {
  const unknownDependencies = Object.keys(dependencies).filter((key) => !DEPENDENCY_KEYS.has(key));
  if (unknownDependencies.length > 0) {
    throw new Error(`Unsupported worker dependencies: ${unknownDependencies.join(', ')}`);
  }
  return {
    profileRegistry: dependencies.profileRegistry || createDeploymentProfileRegistry(),
    artifactResolver: dependencies.artifactResolver || defaultArtifactResolver(),
    artifactReceiptSink: dependencies.artifactReceiptSink || null,
    engineAdapter: dependencies.engineAdapter || createCurrentEngineAdapter(),
    clock: dependencies.clock || (() => Date.now()),
    timers: dependencies.timers || { setTimeout, clearTimeout },
    idFactory: dependencies.idFactory || (() => `legacy-worker-${randomUUID()}`),
  };
}

function assertProfilePlan(profile) {
  if (!profile?.plan || !Object.isFrozen(profile.plan)) {
    throw new Error('DeploymentProfileRegistry must return a frozen canonical plan');
  }
  if (profile.plan.isolation !== 'worktree' && profile.plan.isolation !== 'docker') {
    throw new Error('Deployment profile resolved to non-isolated execution');
  }
}

function assertFrozenProfile(profile) {
  if (
    !Object.isFrozen(profile) ||
    !Object.isFrozen(profile.deployment) ||
    !Object.isFrozen(profile.provider) ||
    !Object.isFrozen(profile.bounds)
  ) {
    throw new Error('DeploymentProfileRegistry must return a frozen deployment descriptor');
  }
}

function assertMatchingProfileHandles(profile, request) {
  if (
    profile.isolationProfile !== request.isolationProfile ||
    profile.providerProfile !== request.providerProfile
  ) {
    throw new Error('DeploymentProfileRegistry returned mismatched profile handles');
  }
}

function assertCanonicalPlan(plan) {
  const canonical = resolveRunPlan({
    docker: plan.isolation === 'docker',
    worktree: plan.isolation === 'worktree',
    pr: plan.delivery === 'pr',
    ship: plan.delivery === 'ship',
  });
  if (
    canonical.isolation !== plan.isolation ||
    canonical.delivery !== plan.delivery ||
    canonical.autoMerge !== plan.autoMerge
  ) {
    throw new Error('DeploymentProfileRegistry returned a non-canonical run plan');
  }
}

function assertProfileBounds(bounds) {
  for (const name of ['executionMs', 'shutdownMs', 'frameBytes']) {
    if (!Number.isSafeInteger(bounds[name]) || bounds[name] <= 0) {
      throw new Error(`Deployment profile bound ${name} must be a positive safe integer`);
    }
  }
}

function assertIsolatedProfile(profile, request) {
  assertProfilePlan(profile);
  assertFrozenProfile(profile);
  assertMatchingProfileHandles(profile, request);
  assertCanonicalPlan(profile.plan);
  assertProfileBounds(profile.bounds);
  return profile;
}

module.exports = {
  assertIsolatedProfile,
  collectReceipts,
  errorOutcome,
  resolveRuntimeDependencies,
  stageArtifacts,
  stopEngineWithinBound,
};
