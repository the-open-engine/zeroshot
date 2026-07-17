'use strict';

const { randomUUID } = require('crypto');
const { validateArtifactRef, validateWorkerOutcome } = require('./contracts');
const { createCurrentEngineAdapter } = require('./engine-adapter');
const { cloneJson, deepFreeze } = require('./object-utils');
const { createDeploymentProfileRegistry } = require('./profiles');
const { resolveRunPlan } = require('../run-plan');

const CANCELLED = Symbol('cancelled');

const DEPENDENCY_KEYS = new Set([
  'profileRegistry',
  'artifactResolver',
  'artifactReceiptSink',
  'engineAdapter',
  'clock',
  'timers',
  'idFactory',
  'cleanupFailureReporter',
]);

async function stageArtifacts(resolver, artifacts, context) {
  if (artifacts.length === 0) return deepFreeze({ artifacts: [] });
  if (context.signal?.aborted) return CANCELLED;
  for (const artifact of artifacts) validateArtifactRef(artifact);
  const stage = typeof resolver === 'function' ? resolver : resolver?.stage;
  if (typeof stage !== 'function') throw new Error('ArtifactResolver must expose stage()');
  const manifest = await runCancellable(
    Promise.resolve().then(() => stage.call(resolver, artifacts, context)),
    {
      signal: context.signal,
      cleanup: (lateManifest) => resolver?.cleanup?.(lateManifest, context),
      reportFailure: context.reportCleanupFailure,
      failureContext: { phase: 'artifact_staging', clusterId: context.clusterId },
    }
  );
  if (manifest === CANCELLED) return CANCELLED;
  if (!manifest || typeof manifest !== 'object' || Array.isArray(manifest)) {
    throw new Error('ArtifactResolver returned an invalid staged manifest');
  }
  return deepFreeze(manifest);
}

function createArtifactPreparation({
  resolver,
  artifacts,
  clusterId,
  getProfile,
  signal,
  reportFailure,
}) {
  return async (isolation) => {
    const manifest = await stageArtifacts(resolver, artifacts, {
      clusterId,
      profile: getProfile(),
      isolation,
      signal,
      reportCleanupFailure: reportFailure,
    });
    if (manifest === CANCELLED) throw new Error('Artifact staging was cancelled');
    return manifest;
  };
}

function defaultCleanupFailureReporter(failure) {
  const message = failure.error instanceof Error ? failure.error.message : String(failure.error);
  const cluster = failure.clusterId ? ` for cluster ${failure.clusterId}` : '';
  process.emitWarning(
    `Late ${failure.phase} ${failure.kind} failed${cluster}: ${message}`,
    'ZeroshotCleanupWarning',
    'ZEROSHOT_LEGACY_CLEANUP_FAILURE'
  );
}

function reportReporterFailure(failure, reporterError) {
  const original = failure.error instanceof Error ? failure.error.message : String(failure.error);
  const reporting = reporterError instanceof Error ? reporterError.message : String(reporterError);
  defaultCleanupFailureReporter({
    ...failure,
    error: new Error(`${original}; cleanup failure reporter also failed: ${reporting}`),
  });
}

function reportLateFailure(reporter, failure) {
  const selected = typeof reporter === 'function' ? reporter : defaultCleanupFailureReporter;
  try {
    const result = selected(Object.freeze(failure));
    if (result && typeof result.then === 'function') {
      result.then(undefined, (reporterError) => reportReporterFailure(failure, reporterError));
    }
  } catch (reporterError) {
    reportReporterFailure(failure, reporterError);
  }
}

async function observeCancelledOperation(operation, { cleanup, reportFailure, failureContext }) {
  let lateValue;
  try {
    lateValue = await operation;
  } catch (error) {
    reportLateFailure(reportFailure, { ...failureContext, kind: 'operation', error });
    return;
  }
  if (typeof cleanup !== 'function') return;
  try {
    await cleanup(lateValue);
  } catch (error) {
    reportLateFailure(reportFailure, { ...failureContext, kind: 'cleanup', error });
  }
}

function observeLateSettlement(operation, options) {
  observeCancelledOperation(operation, options).then(undefined, (error) =>
    reportLateFailure(options.reportFailure, {
      ...options.failureContext,
      kind: 'operation',
      error,
    })
  );
}
async function runCancellable(
  value,
  { signal, cleanup, reportFailure, failureContext = { phase: 'cancelled_operation' } } = {}
) {
  const operation = Promise.resolve(value);
  if (!signal) return operation;
  if (signal.aborted) {
    observeLateSettlement(operation, { cleanup, reportFailure, failureContext });
    return CANCELLED;
  }

  let removeAbortListener = () => {};
  const cancellation = new Promise((resolve) => {
    const onAbort = () => resolve(CANCELLED);
    signal.addEventListener('abort', onAbort, { once: true });
    removeAbortListener = () => signal.removeEventListener('abort', onAbort);
  });
  const completed = operation.then((resolved) => ({ resolved }));
  const winner = await Promise.race([completed, cancellation]);
  removeAbortListener();
  if (winner === CANCELLED) {
    observeLateSettlement(operation, { cleanup, reportFailure, failureContext });
    return CANCELLED;
  }
  return winner.resolved;
}

async function collectReceipts(sink, declared, context) {
  if (!sink) return [];
  if (context.signal?.aborted) return CANCELLED;
  const collect = typeof sink === 'function' ? sink : sink.collect || sink.write;
  if (typeof collect !== 'function') throw new Error('ArtifactReceiptSink must expose collect()');
  const receipts = await runCancellable(
    Promise.resolve().then(() => collect.call(sink, declared || [], context)),
    {
      signal: context.signal,
      cleanup: (lateReceipts) => sink?.cleanup?.(lateReceipts, context),
      reportFailure: context.reportCleanupFailure,
      failureContext: { phase: 'receipt_collection', clusterId: context.clusterId },
    }
  );
  if (receipts === CANCELLED) return CANCELLED;
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
  if (
    dependencies.cleanupFailureReporter !== undefined &&
    typeof dependencies.cleanupFailureReporter !== 'function'
  ) {
    throw new Error('cleanupFailureReporter must be a function');
  }
  return {
    profileRegistry: dependencies.profileRegistry || createDeploymentProfileRegistry(),
    artifactResolver: dependencies.artifactResolver || null,
    artifactReceiptSink: dependencies.artifactReceiptSink || null,
    engineAdapter: dependencies.engineAdapter || createCurrentEngineAdapter(),
    clock: dependencies.clock || (() => Date.now()),
    timers: dependencies.timers || { setTimeout, clearTimeout },
    idFactory: dependencies.idFactory || (() => `legacy-worker-${randomUUID()}`),
    cleanupFailureReporter: dependencies.cleanupFailureReporter || defaultCleanupFailureReporter,
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
  CANCELLED,
  assertIsolatedProfile,
  collectReceipts,
  createArtifactPreparation,
  errorOutcome,
  reportLateFailure,
  resolveRuntimeDependencies,
  runCancellable,
  stageArtifacts,
  stopEngineWithinBound,
};
