'use strict';

const { validateLegacyShipRequest } = require('./contracts');
const { createCurrentEngineAdapter } = require('./engine-adapter');
const { cloneJson, deepFreeze } = require('./object-utils');
const { createDeploymentProfileRegistry, DEFAULT_BOUNDS } = require('./profiles');
const {
  assertIsolatedProfile,
  errorOutcome,
  resolveRuntimeDependencies,
  stageArtifacts,
  stopEngineWithinBound,
} = require('./runtime-support');
const { LegacyWorkerStateMachine } = require('./state-machine');
const { createTerminalNormalizer } = require('./terminal-normalizer');

const CANCELLED = Symbol('cancelled');

class LegacyClusterWorkerRuntime {
  constructor(dependencies) {
    Object.assign(this, resolveRuntimeDependencies(dependencies));
    this.machine = new LegacyWorkerStateMachine({ clock: this.clock });
    this.startClaimed = false;
    this.profile = null;
    this.timeoutHandle = null;
    this.timeoutGeneration = 0;
    this.executionStartedAt = null;
    this.terminalAuthority = null;
    this.engineStartAttempted = false;
    this.engineStopPromise = null;
    this.preparationCancellation = new Promise((resolve) => {
      this.resolvePreparationCancellation = resolve;
    });
    this.terminalClaimed = new Promise((resolve) => {
      this.resolveTerminalClaimed = resolve;
    });
    this.onEngineEvent = createTerminalNormalizer({
      machine: this.machine,
      clock: this.clock,
      artifactReceiptSink: this.artifactReceiptSink,
      getProfile: () => this.profile,
      claimTerminalAuthority: (authority) => this.claimTerminalAuthority(authority),
      failureReceipt: (state, code, reason) => this.failureReceipt(state, code, reason),
      isTerminal: () => this.terminalAuthority !== null || Boolean(this.machine.terminalReceipt),
      terminalClaimed: this.terminalClaimed,
      cancelled: CANCELLED,
    });
  }

  clearExecutionTimeout() {
    this.timeoutGeneration += 1;
    if (this.timeoutHandle !== null) {
      try {
        this.timers.clearTimeout(this.timeoutHandle);
      } catch {
        // The generation guard still makes a stale callback inert.
      }
    }
    this.timeoutHandle = null;
  }

  armExecutionTimeout(milliseconds) {
    this.clearExecutionTimeout();
    const generation = this.timeoutGeneration;
    const elapsed = Math.max(0, this.clock() - this.executionStartedAt);
    const remaining = Math.max(0, milliseconds - elapsed);
    this.timeoutHandle = this.timers.setTimeout(() => {
      if (generation !== this.timeoutGeneration) return;
      this.timeoutHandle = null;
      if (this.claimTerminalAuthority('timeout')) {
        this.machine.terminal(this.failureReceipt('timed_out', 'timeout', 'declared_failure'));
        if (this.engineStartAttempted) this.stopEngine().catch(() => undefined);
      }
    }, remaining);
  }

  initialExecutionBound() {
    const milliseconds =
      this.profile?.bounds.executionMs ?? this.profileRegistry?.bounds?.executionMs;
    return Number.isSafeInteger(milliseconds) && milliseconds > 0
      ? milliseconds
      : DEFAULT_BOUNDS.executionMs;
  }

  cancelPreparation() {
    this.resolvePreparationCancellation(CANCELLED);
  }

  racePreparation(value) {
    return Promise.race([Promise.resolve(value), this.preparationCancellation]);
  }

  failureReceipt(state, code, reason) {
    return {
      state,
      clusterId: this.machine.clusterId,
      finishedAt: this.clock(),
      outcome: errorOutcome(code, reason),
    };
  }

  claimTerminalAuthority(authority) {
    if (this.terminalAuthority !== null || this.machine.terminalReceipt) return false;
    this.terminalAuthority = authority;
    this.clearExecutionTimeout();
    this.cancelPreparation();
    this.resolveTerminalClaimed(CANCELLED);
    return true;
  }

  failEngineStatus() {
    if (this.claimTerminalAuthority('engine:status')) {
      this.machine.terminal(this.failureReceipt('failed', 'crash', 'declared_failure'));
    }
  }

  async prepareStart(request, clusterId, profileResolution) {
    if (!this.profile) {
      const resolvedProfile = await this.racePreparation(profileResolution);
      if (resolvedProfile === CANCELLED) return CANCELLED;
      this.profile = assertIsolatedProfile(resolvedProfile, request);
    }
    this.armExecutionTimeout(this.profile.bounds.executionMs);
    if (this.terminalAuthority !== null) return CANCELLED;
    const artifactManifest = await this.racePreparation(
      stageArtifacts(this.artifactResolver, request.artifacts, {
        clusterId,
        profile: this.profile,
      })
    );
    if (artifactManifest === CANCELLED) return CANCELLED;
    return { clusterId, artifactManifest };
  }

  async startEngine(request, clusterId, artifactManifest) {
    this.engineStartAttempted = true;
    try {
      const engineStart = Promise.resolve(
        this.engineAdapter.start({
          request,
          profile: this.profile,
          artifactManifest,
          clusterId,
          onEvent: this.onEngineEvent,
        })
      );
      const resource = await Promise.race([engineStart, this.terminalClaimed]);
      if (resource === CANCELLED) return this.machine.status();
      if (resource?.clusterId && resource.clusterId !== clusterId) {
        throw new Error('Engine allocated a cluster with a different id');
      }
      if (this.machine.state === 'starting') this.machine.transition('running');
      return this.machine.status();
    } catch (error) {
      if (this.claimTerminalAuthority('engine:start')) {
        this.machine.terminal(this.failureReceipt('failed', 'crash', 'declared_failure'));
      } else if (!this.machine.terminalReceipt) {
        await this.machine.result();
        return this.machine.status();
      }
      throw error;
    }
  }

  beginLifecycle() {
    const clusterId = this.idFactory();
    if (typeof clusterId !== 'string' || !clusterId) {
      throw new Error('idFactory returned invalid id');
    }
    this.machine.setClusterId(clusterId);
    this.machine.transition('starting');
    this.executionStartedAt = this.clock();
    this.armExecutionTimeout(this.initialExecutionBound());
    return clusterId;
  }

  async start(request) {
    if (this.startClaimed) throw new Error('Worker facade permits exactly one start');
    const requestSnapshot = deepFreeze(cloneJson(request));
    validateLegacyShipRequest(requestSnapshot);
    // Claim synchronously so concurrent calls cannot allocate two resources.
    this.startClaimed = true;
    let clusterId;
    try {
      const profileResolution = this.profileRegistry.resolve(
        requestSnapshot.isolationProfile,
        requestSnapshot.providerProfile
      );
      if (!profileResolution || typeof profileResolution.then !== 'function') {
        this.profile = assertIsolatedProfile(profileResolution, requestSnapshot);
      }
      clusterId = this.beginLifecycle();
      const prepared = await this.prepareStart(requestSnapshot, clusterId, profileResolution);
      if (prepared === CANCELLED) return this.machine.status();
      return this.startEngine(requestSnapshot, clusterId, prepared.artifactManifest);
    } catch (error) {
      if (this.terminalAuthority !== null) {
        if (!this.machine.terminalReceipt) await this.machine.result();
        return this.machine.status();
      }
      if (this.machine.clusterId && this.claimTerminalAuthority('engine:start-setup')) {
        this.machine.terminal(this.failureReceipt('failed', 'crash', 'declared_failure'));
      }
      throw error;
    }
  }

  status() {
    if (
      this.engineStartAttempted &&
      this.machine.clusterId &&
      !this.machine.terminalReceipt &&
      typeof this.engineAdapter.status === 'function'
    ) {
      try {
        const diagnostic = this.engineAdapter.status();
        if (diagnostic && typeof diagnostic.then === 'function') {
          diagnostic.then(undefined, () => this.failEngineStatus());
          throw new TypeError('EngineAdapter.status() must return synchronously');
        }
      } catch (error) {
        this.failEngineStatus();
        if (!this.machine.terminalReceipt) throw error;
      }
    }
    return this.machine.status();
  }

  events() {
    return this.machine.events();
  }

  stopEngine() {
    this.engineStopPromise ||= stopEngineWithinBound({
      engineAdapter: this.engineAdapter,
      timers: this.timers,
      shutdownMs: this.profile?.bounds.shutdownMs || DEFAULT_BOUNDS.shutdownMs,
    });
    return this.engineStopPromise;
  }

  async stop() {
    if (!this.startClaimed) throw new Error('Worker has not started');
    if (this.machine.terminalReceipt) {
      if (this.engineStopPromise) await this.engineStopPromise;
      return this.machine.terminalReceipt;
    }
    if (!this.claimTerminalAuthority('stop')) return this.machine.result();
    this.machine.requestStop();
    const effective = this.engineStartAttempted ? await this.stopEngine() : true;
    this.machine.terminal({
      state: 'stopped',
      clusterId: this.machine.clusterId,
      finishedAt: this.clock(),
      stop: {
        requested: true,
        effective,
        externalEffectsRolledBack: false,
      },
    });
    return this.machine.terminalReceipt;
  }

  result() {
    if (!this.startClaimed) throw new Error('Worker has not started');
    return this.machine.result();
  }
}

function createLegacyClusterWorker(dependencies = {}) {
  const runtime = new LegacyClusterWorkerRuntime(dependencies);
  return Object.freeze({
    start: runtime.start.bind(runtime),
    status: runtime.status.bind(runtime),
    events: runtime.events.bind(runtime),
    stop: runtime.stop.bind(runtime),
    result: runtime.result.bind(runtime),
  });
}

module.exports = {
  createLegacyClusterWorker,
  createDeploymentProfileRegistry,
  createCurrentEngineAdapter,
};
