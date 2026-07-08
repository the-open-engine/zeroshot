const assert = require('assert');
const { getProvider } = require('../src/providers');

// Regression coverage for #162: `zeroshot logs` (and any model-spec resolution)
// crashed with `Level "level1" is below minLevel "level3"` when a provider's
// min/max level guardrails were clamped to a single level. minLevel/maxLevel are
// the user's floor/ceiling cost guardrails, so an out-of-range template-pinned
// level must clamp into range instead of throwing.
describe('base provider validateLevel clamping (#162)', function () {
  const provider = getProvider('claude');

  it('clamps a below-floor level up to minLevel', function () {
    assert.strictEqual(provider.validateLevel('level1', 'level3', 'level3'), 'level3');
    assert.strictEqual(provider.validateLevel('level1', 'level2', 'level3'), 'level2');
  });

  it('clamps an above-ceiling level down to maxLevel', function () {
    assert.strictEqual(provider.validateLevel('level3', 'level1', 'level1'), 'level1');
    assert.strictEqual(provider.validateLevel('level3', 'level1', 'level2'), 'level2');
  });

  it('single-level clamp (min === max) forces that level for any input', function () {
    for (const level of ['level1', 'level2', 'level3']) {
      assert.strictEqual(provider.validateLevel(level, 'level2', 'level2'), 'level2');
    }
  });

  it('returns an in-range level unchanged', function () {
    assert.strictEqual(provider.validateLevel('level2', 'level1', 'level3'), 'level2');
  });

  it('still throws on an unknown level key', function () {
    assert.throws(() => provider.validateLevel('level9', 'level1', 'level3'), /Invalid level/);
  });

  it('still throws when minLevel exceeds maxLevel (genuine misconfiguration)', function () {
    assert.throws(
      () => provider.validateLevel('level2', 'level3', 'level1'),
      /minLevel "level3" exceeds maxLevel "level1"/
    );
  });
});
