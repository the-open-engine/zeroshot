const assert = require('node:assert');

const { getConfig } = require('../../src/config-router');

describe('config-router pr mode', function () {
  const originalPrEnv = process.env.ZEROSHOT_PR;

  afterEach(function () {
    if (originalPrEnv === undefined) {
      delete process.env.ZEROSHOT_PR;
      return;
    }

    process.env.ZEROSHOT_PR = originalPrEnv;
  });

  it('routes trivial task through worker-validator when autoPr is enabled', function () {
    const { base, params } = getConfig('TRIVIAL', 'TASK', { autoPr: true });

    assert.strictEqual(base, 'worker-validator');
    assert.strictEqual(params.worker_level, 'level1');
    assert.strictEqual(params.validator_level, 'level1');
  });

  it('routes trivial task through worker-validator when ZEROSHOT_PR=1', function () {
    process.env.ZEROSHOT_PR = '1';

    const { base, params } = getConfig('TRIVIAL', 'TASK');

    assert.strictEqual(base, 'worker-validator');
    assert.strictEqual(params.worker_level, 'level1');
    assert.strictEqual(params.validator_level, 'level1');
  });
});
