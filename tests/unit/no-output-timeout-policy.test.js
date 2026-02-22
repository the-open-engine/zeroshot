const assert = require('assert');

const {
  determineNoOutputTimeoutAction,
  calculateNoOutputHardTimeoutMs,
} = require('../../src/agent/agent-task-executor');

describe('no-output timeout policy', () => {
  it('probes health only when platform and pid are available', () => {
    assert.strictEqual(
      determineNoOutputTimeoutAction({ platformSupported: true, processPid: 1234 }),
      'probe'
    );
  });

  it('fails fast when platform does not support health probe', () => {
    assert.strictEqual(
      determineNoOutputTimeoutAction({ platformSupported: false, processPid: 1234 }),
      'fail'
    );
  });

  it('fails fast when process pid is missing', () => {
    assert.strictEqual(
      determineNoOutputTimeoutAction({ platformSupported: true, processPid: null }),
      'fail'
    );
  });

  it('enforces hard timeout floor and multiplier', () => {
    assert.strictEqual(calculateNoOutputHardTimeoutMs(120000), 720000);
    assert.strictEqual(calculateNoOutputHardTimeoutMs(1000), 600000);
    assert.strictEqual(calculateNoOutputHardTimeoutMs(300000), 1800000);
  });
});
