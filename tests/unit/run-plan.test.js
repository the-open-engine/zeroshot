/**
 * Test: resolveRunPlan
 *
 * Verifies the single canonical isolation/delivery/autoMerge resolver
 * for all combinations of worktree/docker/pr/ship flags.
 */

const assert = require('assert');
const { resolveRunPlan } = require('../../lib/run-plan');

describe('resolveRunPlan', function () {
  it('resolves {} to no isolation, no delivery', function () {
    assert.deepStrictEqual(resolveRunPlan({}), {
      isolation: 'none',
      delivery: 'none',
      autoMerge: false,
    });
  });

  it('resolves {worktree: true}', function () {
    assert.deepStrictEqual(resolveRunPlan({ worktree: true }), {
      isolation: 'worktree',
      delivery: 'none',
      autoMerge: false,
    });
  });

  it('resolves {docker: true}', function () {
    assert.deepStrictEqual(resolveRunPlan({ docker: true }), {
      isolation: 'docker',
      delivery: 'none',
      autoMerge: false,
    });
  });

  it('resolves {pr: true}', function () {
    assert.deepStrictEqual(resolveRunPlan({ pr: true }), {
      isolation: 'worktree',
      delivery: 'pr',
      autoMerge: false,
    });
  });

  it('resolves {ship: true}', function () {
    assert.deepStrictEqual(resolveRunPlan({ ship: true }), {
      isolation: 'worktree',
      delivery: 'ship',
      autoMerge: true,
    });
  });

  it('resolves {pr: true, docker: true}', function () {
    assert.deepStrictEqual(resolveRunPlan({ pr: true, docker: true }), {
      isolation: 'docker',
      delivery: 'pr',
      autoMerge: false,
    });
  });

  it('resolves {ship: true, docker: true}', function () {
    assert.deepStrictEqual(resolveRunPlan({ ship: true, docker: true }), {
      isolation: 'docker',
      delivery: 'ship',
      autoMerge: true,
    });
  });

  it('returns a frozen object', function () {
    assert.strictEqual(Object.isFrozen(resolveRunPlan({})), true);
  });

  it('defaults options to {} when called with no arguments', function () {
    assert.deepStrictEqual(resolveRunPlan(), {
      isolation: 'none',
      delivery: 'none',
      autoMerge: false,
    });
  });
});
