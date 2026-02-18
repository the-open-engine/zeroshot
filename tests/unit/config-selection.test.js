const assert = require('assert');
const path = require('path');

const { resolveConfigSelection, resolveConfigPath } = require('../../lib/start-cluster');

describe('config selection', function () {
  it('prefers CLI --config over repo and global defaults', function () {
    const selection = resolveConfigSelection(
      { config: './from-cli.json' },
      { defaultConfig: 'global-default' },
      '/workspace/subdir',
      { repoRoot: '/workspace', settings: { defaultConfig: './from-repo.json' } }
    );

    assert.deepStrictEqual(selection, {
      configName: './from-cli.json',
      baseDir: '/workspace/subdir',
      source: 'cli',
    });
  });

  it('prefers repo-local defaultConfig over global defaultConfig', function () {
    const selection = resolveConfigSelection(
      {},
      { defaultConfig: 'global-default' },
      '/workspace/subdir',
      { repoRoot: '/workspace', settings: { defaultConfig: './from-repo.json' } }
    );

    assert.deepStrictEqual(selection, {
      configName: './from-repo.json',
      baseDir: '/workspace',
      source: 'repo',
    });
  });

  it('uses global defaultConfig when repo-local defaultConfig is missing', function () {
    const selection = resolveConfigSelection(
      {},
      { defaultConfig: 'global-default' },
      '/workspace/subdir',
      { repoRoot: '/workspace', settings: {} }
    );

    assert.deepStrictEqual(selection, {
      configName: 'global-default',
      baseDir: '/workspace/subdir',
      source: 'global',
    });
  });

  it('falls back to conductor-bootstrap when no defaults are configured', function () {
    const selection = resolveConfigSelection({}, {}, '/workspace/subdir', {
      repoRoot: '/workspace',
      settings: {},
    });

    assert.deepStrictEqual(selection, {
      configName: 'conductor-bootstrap',
      baseDir: '/workspace/subdir',
      source: 'fallback',
    });
  });

  it('resolves relative config paths against provided base dir', function () {
    const configPath = resolveConfigPath('./.zeroshot/topologies/security-review.json', '/workspace');
    assert.strictEqual(
      configPath,
      path.resolve('/workspace', './.zeroshot/topologies/security-review.json')
    );
  });

  it('keeps template-name resolution behavior for non-relative names', function () {
    const configPath = resolveConfigPath('conductor-bootstrap', '/workspace');
    assert.strictEqual(
      configPath,
      path.join(path.resolve(__dirname, '..', '..'), 'cluster-templates', 'conductor-bootstrap.json')
    );
  });
});
