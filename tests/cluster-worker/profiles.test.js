'use strict';

const assert = require('assert');
const { createDeploymentProfileRegistry } = require('../../lib/cluster-worker/profiles');

describe('legacy cluster worker deployment profiles', () => {
  it('resolves registry handles to frozen isolated canonical plans', () => {
    const registry = createDeploymentProfileRegistry();
    assert.ok(Object.isFrozen(registry.bounds));
    assert.strictEqual(registry.bounds.executionMs, 60 * 60 * 1000);
    const expected = {
      'isolation.worktree@1': ['worktree', 'none'],
      'isolation.docker@1': ['docker', 'none'],
      'isolation.pr@1': ['worktree', 'pr'],
      'isolation.ship@1': ['worktree', 'ship'],
    };
    for (const [handle, [isolation, delivery]] of Object.entries(expected)) {
      const profile = registry.resolve(handle, 'provider.default@1');
      assert.deepStrictEqual(
        [profile.plan.isolation, profile.plan.delivery],
        [isolation, delivery]
      );
      assert.ok(Object.isFrozen(profile));
      assert.ok(Object.isFrozen(profile.plan));
      assert.ok(Object.isFrozen(profile.provider));
    }
  });

  it('rejects unknown and non-isolated resolutions', () => {
    const registry = createDeploymentProfileRegistry({
      isolationProfiles: { 'isolation.none@1': {} },
    });
    assert.throws(
      () => registry.resolve('isolation.unknown@1', 'provider.default@1'),
      /Unknown isolation profile/
    );
    assert.throws(
      () => registry.resolve('isolation.none@1', 'provider.default@1'),
      /non-isolated execution/
    );
    assert.throws(
      () => registry.resolve('isolation.none@1', 'provider.unknown@1'),
      /Unknown provider profile/
    );
  });

  it('rejects invalid registry-owned bounds', () => {
    assert.throws(() => createDeploymentProfileRegistry({ bounds: { executionMs: 0 } }));
    const registry = createDeploymentProfileRegistry({
      isolationProfiles: { 'isolation.bad@1': { worktree: true, bounds: { frameBytes: -1 } } },
    });
    assert.throws(() => registry.resolve('isolation.bad@1', 'provider.default@1'));
  });
});
