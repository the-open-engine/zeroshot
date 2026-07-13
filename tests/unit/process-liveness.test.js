const assert = require('assert');
const { isProcessRunning } = require('../../lib/process-liveness');

describe('process-liveness', function () {
  it('reports the current process as running', function () {
    assert.strictEqual(isProcessRunning(process.pid), true);
  });

  it('reports a PID that does not exist as not running', function () {
    assert.strictEqual(isProcessRunning(999999), false);
  });

  it('rejects non-PID inputs without throwing', function () {
    assert.strictEqual(isProcessRunning(null), false);
    assert.strictEqual(isProcessRunning(undefined), false);
    assert.strictEqual(isProcessRunning(0), false);
    assert.strictEqual(isProcessRunning(-5), false);
    assert.strictEqual(isProcessRunning(1.5), false);
    assert.strictEqual(isProcessRunning('123'), false);
  });
});
