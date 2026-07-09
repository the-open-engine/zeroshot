/**
 * Test: repo-local settings read/write
 *
 * writeRepoSettings() + readRepoSettings() round-trip against a real
 * `.zeroshot/settings.json` in a temp git repo.
 */

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');
const { execSync } = require('child_process');

const { readRepoSettings, writeRepoSettings } = require('../../lib/repo-settings');

describe('repo-settings', function () {
  this.timeout(10000);

  let repoRoot;

  beforeEach(function () {
    repoRoot = path.join(
      os.tmpdir(),
      'zeroshot-repo-settings-test-' + crypto.randomBytes(8).toString('hex')
    );
    fs.mkdirSync(repoRoot, { recursive: true });
    execSync('git init', { cwd: repoRoot, stdio: 'ignore' });
  });

  afterEach(function () {
    fs.rmSync(repoRoot, { recursive: true, force: true });
  });

  it('writeRepoSettings creates .zeroshot/settings.json', function () {
    const settingsPath = writeRepoSettings(repoRoot, { prBase: 'main' });
    assert.strictEqual(settingsPath, path.join(repoRoot, '.zeroshot', 'settings.json'));
    assert.ok(fs.existsSync(settingsPath));
    const onDisk = JSON.parse(fs.readFileSync(settingsPath, 'utf8'));
    assert.deepStrictEqual(onDisk, { prBase: 'main' });
  });

  it('readRepoSettings round-trips what writeRepoSettings wrote', function () {
    writeRepoSettings(repoRoot, { prBase: 'dev', dockerMounts: ['gh', 'git'] });
    const { repoRoot: detectedRoot, settings, settingsPath } = readRepoSettings(repoRoot);
    // git resolves symlinks (e.g. macOS /tmp -> /private/tmp), so compare via realpath.
    assert.strictEqual(
      settingsPath,
      path.join(fs.realpathSync(repoRoot), '.zeroshot', 'settings.json')
    );
    assert.ok(detectedRoot);
    assert.deepStrictEqual(settings, { prBase: 'dev', dockerMounts: ['gh', 'git'] });
  });

  it('writeRepoSettings overwrites a previous file', function () {
    writeRepoSettings(repoRoot, { prBase: 'main' });
    writeRepoSettings(repoRoot, { prBase: 'dev' });
    const { settings } = readRepoSettings(repoRoot);
    assert.deepStrictEqual(settings, { prBase: 'dev' });
  });

  it('readRepoSettings returns null settings when no file exists yet', function () {
    const { settings, repoRoot: detectedRoot } = readRepoSettings(repoRoot);
    assert.strictEqual(settings, null);
    assert.ok(detectedRoot);
  });
});
