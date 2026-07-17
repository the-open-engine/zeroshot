'use strict';

const assert = require('assert');
const {
  MAX_SUMMARY_BYTES,
  validateCommandFrame,
  validateLegacyShipRequest,
  validateLegacyShipResult,
  validateWorkerOutcome,
} = require('../../lib/cluster-worker/contracts');
const { ARTIFACT, request } = require('./helpers');

describe('legacy cluster worker contracts', () => {
  it('accepts exactly the canonical issue, prompt, and artifact source variants', () => {
    for (const source of ['issue', 'prompt', 'artifact']) {
      assert.strictEqual(validateLegacyShipRequest(request(source)).source, source);
    }
  });

  it('rejects inconsistent source fields and empty artifact input', () => {
    assert.throws(() => validateLegacyShipRequest({ ...request('issue'), prompt: 'also prompt' }));
    assert.throws(() => validateLegacyShipRequest({ ...request('prompt'), issue: 'also issue' }));
    assert.throws(() => validateLegacyShipRequest({ ...request('artifact'), artifacts: [] }));
    assert.throws(() => validateLegacyShipRequest({ ...request('issue'), issue: '   ' }));
  });

  it('rejects credentials, commands, paths, models, timeouts, flags, and unknown fields', () => {
    for (const field of [
      'credential',
      'token',
      'command',
      'cwd',
      'path',
      'model',
      'timeout',
      'launchFlags',
      'environment',
      'endpoint',
    ]) {
      assert.throws(
        () => validateLegacyShipRequest({ ...request(), [field]: 'caller-controlled' }),
        /Invalid LegacyShipRequest/
      );
    }
  });

  it('validates byte-free artifacts and closed worker error pairs', () => {
    assert.strictEqual(validateLegacyShipRequest(request('artifact')).artifacts[0], ARTIFACT);
    assert.throws(() =>
      validateLegacyShipRequest({
        ...request('artifact'),
        artifacts: [{ ...ARTIFACT, bytes: 'secret' }],
      })
    );
    assert.doesNotThrow(() =>
      validateWorkerOutcome({ status: 'error', code: 'refusal', reason: 'policy_denied' })
    );
    assert.throws(() =>
      validateWorkerOutcome({ status: 'error', code: 'crash', reason: 'policy_denied' })
    );
  });

  it('bounds result summaries by UTF-8 bytes', () => {
    assert.doesNotThrow(() =>
      validateLegacyShipResult({
        summary: 'x'.repeat(MAX_SUMMARY_BYTES),
        status: 'succeeded',
        artifacts: [],
      })
    );
    assert.throws(() =>
      validateLegacyShipResult({
        summary: 'x'.repeat(MAX_SUMMARY_BYTES + 1),
        status: 'succeeded',
        artifacts: [],
      })
    );
  });

  it('accepts only the five closed command frames', () => {
    assert.doesNotThrow(() =>
      validateCommandFrame({ id: 'start-1', method: 'start', params: { request: request() } })
    );
    for (const method of ['status', 'events', 'stop', 'result']) {
      assert.doesNotThrow(() => validateCommandFrame({ id: method, method, params: {} }));
    }
    assert.throws(() => validateCommandFrame([]));
    assert.throws(() => validateCommandFrame({ id: 'x', method: 'attach', params: {} }));
    assert.throws(() =>
      validateCommandFrame({ id: 'x', method: 'status', params: { writable: true } })
    );
  });
});
