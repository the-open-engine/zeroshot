/**
 * Test: IsolationManager._getGhToken() respects GH_CONFIG_DIR
 *
 * Regression tests for the bug where _getGhToken() hardcoded ~/.config/gh/hosts.yml
 * and ignored the GH_CONFIG_DIR environment variable. In environments where gh is
 * configured via GH_CONFIG_DIR (e.g. Kubernetes pods with mounted gh config), the
 * token lookup failed and git push produced a "No such device or address" TTY error.
 */

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const IsolationManager = require('../../src/isolation-manager');

describe('IsolationManager._getGhToken', function () {
  let tempDir;
  let savedGhConfigDir;
  let savedPath;

  beforeEach(function () {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-gh-token-test-'));
    savedGhConfigDir = process.env.GH_CONFIG_DIR;
    savedPath = process.env.PATH;
  });

  afterEach(function () {
    fs.rmSync(tempDir, { recursive: true, force: true });
    if (savedGhConfigDir === undefined) {
      delete process.env.GH_CONFIG_DIR;
    } else {
      process.env.GH_CONFIG_DIR = savedGhConfigDir;
    }
    process.env.PATH = savedPath;
  });

  it('reads token from GH_CONFIG_DIR/hosts.yml when gh CLI is unavailable', function () {
    // Put a known token in a temp dir that is NOT the default ~/.config/gh
    const hostsYml = 'github.com:\n  oauth_token: sentinel-token-env-dir\n  user: testuser\n';
    fs.writeFileSync(path.join(tempDir, 'hosts.yml'), hostsYml, 'utf8');

    // Point GH_CONFIG_DIR at the temp dir and remove gh from PATH so the
    // `gh auth token` step fails and we exercise the file-based fallback.
    process.env.GH_CONFIG_DIR = tempDir;
    process.env.PATH = '/nonexistent-path-for-test';

    const manager = new IsolationManager();
    const token = manager._getGhToken();

    assert.strictEqual(token, 'sentinel-token-env-dir');
  });

  it('returns null when GH_CONFIG_DIR hosts.yml is missing and gh CLI is unavailable', function () {
    // GH_CONFIG_DIR points to an empty temp dir (no hosts.yml)
    process.env.GH_CONFIG_DIR = tempDir;
    process.env.PATH = '/nonexistent-path-for-test';

    const manager = new IsolationManager();
    const token = manager._getGhToken();

    assert.strictEqual(token, null);
  });

  it('uses GH_CONFIG_DIR over the default ~/.config/gh path', function () {
    // Write different tokens to the env-var path and the home-relative path
    const envDirToken = 'token-from-env-dir';
    fs.writeFileSync(
      path.join(tempDir, 'hosts.yml'),
      `github.com:\n  oauth_token: ${envDirToken}\n`,
      'utf8'
    );

    // Create a competing hosts.yml in a second temp dir to simulate ~/.config/gh
    const homeFakeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-fake-home-'));
    try {
      const defaultGhDir = path.join(homeFakeDir, '.config', 'gh');
      fs.mkdirSync(defaultGhDir, { recursive: true });
      fs.writeFileSync(
        path.join(defaultGhDir, 'hosts.yml'),
        'github.com:\n  oauth_token: token-from-default-dir\n',
        'utf8'
      );

      process.env.GH_CONFIG_DIR = tempDir;
      process.env.PATH = '/nonexistent-path-for-test';

      const manager = new IsolationManager();
      const token = manager._getGhToken();

      assert.strictEqual(token, envDirToken, 'Should use GH_CONFIG_DIR, not ~/.config/gh');
    } finally {
      fs.rmSync(homeFakeDir, { recursive: true, force: true });
    }
  });
});
