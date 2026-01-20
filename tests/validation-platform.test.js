const assert = require('assert');
const {
  isPlatformMismatchReason,
  findPlatformMismatchReason,
} = require('../src/agent/validation-platform');

describe('Validation platform mismatch detection', function () {
  it('detects platform mismatch from reason strings', function () {
    assert.ok(isPlatformMismatchReason('EBADPLATFORM @esbuild/linux-x64'));
    assert.ok(isPlatformMismatchReason('Unsupported platform for @esbuild/linux-x64'));
    assert.ok(isPlatformMismatchReason('darwin-arm64 vs linux-x64'));
    assert.ok(!isPlatformMismatchReason('kubectl not installed'));
  });

  it('finds platform mismatch in criteriaResults', function () {
    const result = {
      criteriaResults: [
        { id: 'AC1', status: 'PASS' },
        {
          id: 'AC2',
          status: 'CANNOT_VALIDATE',
          reason: 'npm install fails on darwin-arm64 (EBADPLATFORM for @esbuild/linux-x64)',
        },
      ],
    };

    const reason = findPlatformMismatchReason(result);
    assert.ok(reason, 'Should return a platform mismatch reason');
    assert.ok(reason.includes('EBADPLATFORM'), 'Should keep original reason');
  });

  it('finds platform mismatch in errors array', function () {
    const result = {
      errors: ['EBADPLATFORM for @esbuild/linux-x64'],
    };

    const reason = findPlatformMismatchReason(result);
    assert.ok(reason, 'Should return a platform mismatch reason');
  });

  it('returns null when no mismatch found', function () {
    const result = {
      criteriaResults: [{ id: 'AC1', status: 'CANNOT_VALIDATE', reason: 'kubectl not installed' }],
      errors: ['No SSH access'],
    };

    assert.strictEqual(findPlatformMismatchReason(result), null);
  });
});
