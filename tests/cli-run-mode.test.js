const assert = require('assert');
const { resolveRunMode } = require('../cli/index.js');

describe('resolveRunMode', () => {
  it('returns "ship" when options.ship is set', () => {
    assert.strictEqual(resolveRunMode({ ship: true }), 'ship');
  });

  it('returns "ship+docker" when ship and docker are both set', () => {
    assert.strictEqual(resolveRunMode({ ship: true, docker: true }), 'ship+docker');
  });

  it('returns "pr" when options.pr is set', () => {
    assert.strictEqual(resolveRunMode({ pr: true }), 'pr');
  });

  it('returns "pr+docker" when pr and docker are both set', () => {
    assert.strictEqual(resolveRunMode({ pr: true, docker: true }), 'pr+docker');
  });

  it('returns "docker" when only options.docker is set', () => {
    assert.strictEqual(resolveRunMode({ docker: true }), 'docker');
  });

  it('returns "worktree" when only options.worktree is set', () => {
    assert.strictEqual(resolveRunMode({ worktree: true }), 'worktree');
  });

  it('returns null when no mode flags are set', () => {
    assert.strictEqual(resolveRunMode({}), null);
  });

  it('prioritizes ship over pr, docker, and worktree', () => {
    assert.strictEqual(
      resolveRunMode({ ship: true, pr: true, docker: true, worktree: true }),
      'ship+docker'
    );
  });
});
