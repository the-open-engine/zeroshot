'use strict';

const { validateLegacyShipResult, validateWorkerOutcome } = require('./contracts');
const { cloneJson, deepFreeze } = require('./object-utils');
const { collectReceipts } = require('./runtime-support');

async function resultFromEvent(event, options) {
  let summary;
  let status = 'succeeded';
  let declaredArtifacts = event.artifacts;
  if (Object.prototype.hasOwnProperty.call(event, 'result')) {
    validateLegacyShipResult(event.result);
    summary = event.result.summary;
    status = event.result.status;
    declaredArtifacts = event.result.artifacts;
  } else {
    summary = event.summary;
    validateLegacyShipResult({ summary, status, artifacts: [] });
  }
  const artifacts = await collectReceipts(options.artifactReceiptSink, declaredArtifacts, {
    clusterId: options.machine.clusterId,
    profile: options.getProfile(),
  });
  const result = { summary, status, artifacts };
  validateLegacyShipResult(result);
  return result;
}

function createTerminalNormalizer(options) {
  let completionPending = false;

  function terminalFailure(authority, state, code, reason) {
    if (options.claimTerminalAuthority(authority)) {
      options.machine.terminal(options.failureReceipt(state, code, reason));
    }
  }

  async function normalizeCompletion(event) {
    try {
      const result = await Promise.race([resultFromEvent(event, options), options.terminalClaimed]);
      if (result === options.cancelled) return;
      if (options.claimTerminalAuthority('engine:complete')) {
        options.machine.terminal({
          state: 'completed',
          clusterId: options.machine.clusterId,
          finishedAt: options.clock(),
          result: deepFreeze(cloneJson(result)),
        });
      }
    } catch {
      terminalFailure('engine:complete:malformed', 'malformed', 'malformed', 'malformed_result');
    } finally {
      completionPending = false;
    }
  }

  function normalizeFailure(event) {
    try {
      validateWorkerOutcome({ status: 'error', code: event.code, reason: event.reason });
      terminalFailure('engine:failed', 'failed', event.code, event.reason);
    } catch {
      terminalFailure('engine:failed:malformed', 'malformed', 'malformed', 'malformed_result');
    }
  }

  function beginCompletion(event) {
    if (completionPending) return;
    completionPending = true;
    normalizeCompletion(event).catch(() => undefined);
  }

  const terminalHandlers = {
    failed: normalizeFailure,
    malformed() {
      terminalFailure('engine:malformed', 'malformed', 'malformed', 'malformed_result');
    },
  };

  return function onEngineEvent(event) {
    if (!event || typeof event !== 'object' || Array.isArray(event)) return;
    if (event.type === 'running') {
      if (options.machine.state === 'starting') options.machine.transition('running');
      return;
    }
    if (options.isTerminal()) return;
    if (event.type === 'complete') return beginCompletion(event);
    if (completionPending) return;
    terminalHandlers[event.type]?.(event);
  };
}

module.exports = { createTerminalNormalizer };
