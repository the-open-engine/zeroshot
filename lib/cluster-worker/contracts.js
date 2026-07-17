'use strict';

const path = require('path');
const Ajv2020 = require('ajv/dist/2020');
const workerSchema = require(
  path.join('..', '..', 'protocol', 'openengine-cluster', 'v1', 'worker.schema.json')
);

const MAX_SUMMARY_BYTES = 4096;
const METHODS = Object.freeze(['start', 'status', 'events', 'stop', 'result']);
const TERMINAL_STATES = Object.freeze(['completed', 'failed', 'timed_out', 'stopped', 'malformed']);

const ajv = new Ajv2020({ allErrors: true, strict: false, allowUnionTypes: true });

function compileDefinition(name) {
  if (!workerSchema.$defs[name]) {
    throw new Error(`Worker schema definition ${name} is missing`);
  }
  return ajv.compile({ $ref: `#/$defs/${name}`, $defs: workerSchema.$defs });
}

const validators = Object.freeze({
  LegacyShipRequest: compileDefinition('LegacyShipRequest'),
  LegacyShipResult: compileDefinition('LegacyShipResult'),
  WorkerOutcome: compileDefinition('WorkerOutcome'),
  ArtifactRef: compileDefinition('ArtifactRef'),
});

function formatErrors(errors) {
  return (errors || []).map((error) => `${error.instancePath || '/'} ${error.message}`).join('; ');
}

function assertDefinition(name, value) {
  const validate = validators[name];
  if (!validate(value)) {
    const error = new TypeError(`Invalid ${name}: ${formatErrors(validate.errors)}`);
    error.code = 'INVALID_CONTRACT';
    error.validationErrors = validate.errors;
    throw error;
  }
  return value;
}

function byteLength(value) {
  return Buffer.byteLength(value, 'utf8');
}

function assertBoundedSummary(summary) {
  if (typeof summary !== 'string' || byteLength(summary) > MAX_SUMMARY_BYTES) {
    throw new TypeError(`Summary must be a string of at most ${MAX_SUMMARY_BYTES} UTF-8 bytes`);
  }
  return summary;
}

function validateLegacyShipRequest(value) {
  return assertDefinition('LegacyShipRequest', value);
}

function validateLegacyShipResult(value) {
  assertDefinition('LegacyShipResult', value);
  assertBoundedSummary(value.summary);
  return value;
}

function validateWorkerOutcome(value) {
  return assertDefinition('WorkerOutcome', value);
}

function validateArtifactRef(value) {
  return assertDefinition('ArtifactRef', value);
}

function assertPlainObject(value, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`${label} must be an object`);
  }
}

function assertExactKeys(value, allowed, label) {
  const unknown = Object.keys(value).filter((key) => !allowed.includes(key));
  if (unknown.length > 0) {
    throw new TypeError(`${label} contains unknown fields: ${unknown.join(', ')}`);
  }
}

function validateCommandFrame(value) {
  assertPlainObject(value, 'Command frame');
  assertExactKeys(value, ['id', 'method', 'params'], 'Command frame');
  if (
    !(
      (typeof value.id === 'string' && value.id.length > 0 && value.id.length <= 128) ||
      (Number.isSafeInteger(value.id) && value.id >= 0)
    )
  ) {
    throw new TypeError(
      'Command frame id must be a nonempty string up to 128 characters or a nonnegative integer'
    );
  }
  if (!METHODS.includes(value.method)) {
    throw new TypeError(`Unknown worker method: ${String(value.method)}`);
  }
  assertPlainObject(value.params, 'Command frame params');
  if (value.method === 'start') {
    assertExactKeys(value.params, ['request'], 'start params');
    if (!Object.prototype.hasOwnProperty.call(value.params, 'request')) {
      throw new TypeError('start params require request');
    }
    validateLegacyShipRequest(value.params.request);
  } else {
    assertExactKeys(value.params, [], `${value.method} params`);
  }
  return value;
}

function validateLifecycleStatus(value) {
  assertPlainObject(value, 'Lifecycle status');
  assertExactKeys(
    value,
    ['state', 'clusterId', 'sequence', 'stopRequested', 'terminal'],
    'Lifecycle status'
  );
  const states = ['idle', 'starting', 'running', 'stopping', ...TERMINAL_STATES];
  if (!states.includes(value.state)) throw new TypeError(`Invalid lifecycle state: ${value.state}`);
  if (value.clusterId !== null && typeof value.clusterId !== 'string') {
    throw new TypeError('Lifecycle clusterId must be a string or null');
  }
  if (!Number.isSafeInteger(value.sequence) || value.sequence < 0) {
    throw new TypeError('Lifecycle sequence must be a nonnegative integer');
  }
  if (typeof value.stopRequested !== 'boolean') {
    throw new TypeError('Lifecycle stopRequested must be boolean');
  }
  if (value.terminal !== TERMINAL_STATES.includes(value.state)) {
    throw new TypeError('Lifecycle terminal flag does not match state');
  }
  return value;
}

function validateCompletedReceipt(value) {
  assertExactKeys(value, ['state', 'clusterId', 'finishedAt', 'result'], 'Terminal receipt');
  validateLegacyShipResult(value.result);
}

function validateStoppedReceipt(value) {
  assertExactKeys(value, ['state', 'clusterId', 'finishedAt', 'stop'], 'Terminal receipt');
  assertPlainObject(value.stop, 'Stop receipt');
  assertExactKeys(
    value.stop,
    ['requested', 'effective', 'externalEffectsRolledBack'],
    'Stop receipt'
  );
  const valid =
    value.stop.requested === true &&
    typeof value.stop.effective === 'boolean' &&
    value.stop.externalEffectsRolledBack === false;
  if (!valid) throw new TypeError('Invalid stop receipt');
}

function validateFailureReceipt(value) {
  assertExactKeys(value, ['state', 'clusterId', 'finishedAt', 'outcome'], 'Terminal receipt');
  validateWorkerOutcome(value.outcome);
  if (value.outcome.status !== 'error')
    throw new TypeError('Failure receipt requires error outcome');
}

function validateTerminalReceipt(value) {
  assertPlainObject(value, 'Terminal receipt');
  if (!TERMINAL_STATES.includes(value.state)) {
    throw new TypeError(`Invalid terminal receipt state: ${value.state}`);
  }
  if (typeof value.clusterId !== 'string' || !value.clusterId) {
    throw new TypeError('Terminal receipt requires clusterId');
  }
  if (!Number.isFinite(value.finishedAt)) {
    throw new TypeError('Terminal receipt requires numeric finishedAt');
  }
  if (value.state === 'completed') validateCompletedReceipt(value);
  else if (value.state === 'stopped') validateStoppedReceipt(value);
  else validateFailureReceipt(value);
  return value;
}

module.exports = {
  MAX_SUMMARY_BYTES,
  METHODS,
  TERMINAL_STATES,
  validateLegacyShipRequest,
  validateLegacyShipResult,
  validateWorkerOutcome,
  validateArtifactRef,
  validateCommandFrame,
  validateLifecycleStatus,
  validateTerminalReceipt,
};
