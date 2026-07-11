const assert = require('assert');

const { latestReleaseTag } = require('../scripts/assert-release-published');

describe('release publication assertion', () => {
  it('selects the highest semver release tag on HEAD', () => {
    assert.strictEqual(latestReleaseTag(['v6.5.0', 'v6.6.0']), 'v6.6.0');
    assert.strictEqual(latestReleaseTag(['v6.10.0', 'v6.9.9']), 'v6.10.0');
  });

  it('ignores non-release tags', () => {
    assert.strictEqual(latestReleaseTag(['nightly', 'v6.6.0-beta.1', 'v6.6.0']), 'v6.6.0');
    assert.strictEqual(latestReleaseTag(['nightly']), null);
  });
});
