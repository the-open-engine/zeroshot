const assert = require('assert');

const {
  analyzeMessage,
  maxReleaseType,
  validateReleaseConfig,
} = require('../scripts/release-preflight');

describe('release preflight', () => {
  it('classifies release promotion commits as minor releases', () => {
    assert.strictEqual(analyzeMessage('release: promote dev to main'), 'minor');
    assert.strictEqual(analyzeMessage('release(main): promote dev to main'), 'minor');
  });

  it('classifies conventional breaking commits as majors', () => {
    assert.strictEqual(analyzeMessage('feat!: replace release flow'), 'major');
    assert.strictEqual(
      analyzeMessage('fix: repair release\n\nBREAKING CHANGE: config moved'),
      'major'
    );
  });

  it('preserves the highest release type found', () => {
    assert.strictEqual(maxReleaseType('patch', 'minor'), 'minor');
    assert.strictEqual(maxReleaseType('minor', 'patch'), 'minor');
    assert.strictEqual(maxReleaseType('minor', 'major'), 'major');
  });

  it('rejects branch-writing plugins in the effective release config', () => {
    assert.throws(
      () =>
        validateReleaseConfig({
          release: {
            branches: ['main'],
            plugins: [
              '@semantic-release/commit-analyzer',
              '@semantic-release/release-notes-generator',
              ['@semantic-release/npm', { npmPublish: true }],
              '@semantic-release/git',
              '@semantic-release/github',
            ],
          },
        }),
      /must not be in the effective release config/
    );
  });

  it('accepts the protected-main release config', () => {
    const plugins = validateReleaseConfig({
      release: {
        branches: ['main'],
        plugins: [
          [
            '@semantic-release/commit-analyzer',
            { releaseRules: [{ type: 'release', release: 'minor' }] },
          ],
          '@semantic-release/release-notes-generator',
          ['@semantic-release/npm', { npmPublish: true }],
          '@semantic-release/github',
        ],
      },
    });

    assert.deepStrictEqual(plugins, [
      '@semantic-release/commit-analyzer',
      '@semantic-release/release-notes-generator',
      '@semantic-release/npm',
      '@semantic-release/github',
    ]);
  });
});
