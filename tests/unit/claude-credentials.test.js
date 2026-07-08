/**
 * Regression tests for macOS Keychain credential propagation into isolated
 * CLAUDE_CONFIG_DIRs (GitHub issue #544).
 *
 * Without this module, a Keychain-only (OAuth subscription) login on macOS
 * with no ~/.claude/.credentials.json file left every isolated agent
 * (--worktree / --docker) credential-less, failing with "Not logged in".
 */
const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const sinon = require('sinon');

const safeExec = require('../../src/lib/safe-exec');

function withPlatform(value, fn) {
  const original = Object.getOwnPropertyDescriptor(os, 'platform');
  Object.defineProperty(os, 'platform', { value: () => value, configurable: true });
  try {
    return fn();
  } finally {
    Object.defineProperty(os, 'platform', original);
  }
}

describe('claude-credentials', function () {
  /** @type {sinon.SinonStub} */
  let execSyncStub;
  let claudeCredentials;
  /** @type {string[]} */
  let tempDirs = [];

  beforeEach(function () {
    // Stub before requiring so the module's destructured `execSync` is the stub.
    execSyncStub = sinon.stub(safeExec, 'execSync');
    delete require.cache[require.resolve('../../src/claude-credentials.js')];
    claudeCredentials = require('../../src/claude-credentials.js');
  });

  afterEach(function () {
    sinon.restore();
    delete require.cache[require.resolve('../../src/claude-credentials.js')];
    for (const dir of tempDirs.splice(0)) {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  describe('readKeychainCredentials()', function () {
    it('returns null on non-darwin without invoking the Keychain', function () {
      const result = withPlatform('linux', () => claudeCredentials.readKeychainCredentials());
      assert.strictEqual(result, null);
      assert.strictEqual(execSyncStub.called, false);
    });

    it('returns the credentials JSON when the Keychain has a valid entry', function () {
      execSyncStub.returns('{"claudeAiOauth":{"accessToken":"tok"}}\n');
      const result = withPlatform('darwin', () => claudeCredentials.readKeychainCredentials());
      assert.strictEqual(result, '{"claudeAiOauth":{"accessToken":"tok"}}');
    });

    it('returns null when the Keychain command throws (no item / locked)', function () {
      execSyncStub.throws(new Error('security: item not found'));
      const result = withPlatform('darwin', () => claudeCredentials.readKeychainCredentials());
      assert.strictEqual(result, null);
    });

    it('returns null when the Keychain returns non-JSON garbage', function () {
      execSyncStub.returns('not json');
      const result = withPlatform('darwin', () => claudeCredentials.readKeychainCredentials());
      assert.strictEqual(result, null);
    });
  });

  describe('provisionClaudeCredentials()', function () {
    function makeDirs() {
      const sourceDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-creds-source-'));
      const destDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-creds-dest-'));
      tempDirs.push(sourceDir, destDir);
      return { sourceDir, destDir };
    }

    it('copies an existing source credentials file without consulting the Keychain', function () {
      const { sourceDir, destDir } = makeDirs();
      fs.writeFileSync(path.join(sourceDir, '.credentials.json'), '{"apiKey":"file-creds"}\n');

      const result = withPlatform('darwin', () =>
        claudeCredentials.provisionClaudeCredentials({ sourceDir, destDir })
      );

      assert.strictEqual(result, true);
      assert.strictEqual(execSyncStub.called, false);
      assert.strictEqual(
        fs.readFileSync(path.join(destDir, '.credentials.json'), 'utf8'),
        '{"apiKey":"file-creds"}\n'
      );
    });

    it('materializes Keychain credentials into destDir when no source file exists', function () {
      const { sourceDir, destDir } = makeDirs();
      execSyncStub.returns('{"claudeAiOauth":{"accessToken":"tok"}}');

      const result = withPlatform('darwin', () =>
        claudeCredentials.provisionClaudeCredentials({ sourceDir, destDir })
      );

      assert.strictEqual(result, true);
      assert.strictEqual(
        fs.readFileSync(path.join(destDir, '.credentials.json'), 'utf8'),
        '{"claudeAiOauth":{"accessToken":"tok"}}'
      );
    });

    it('writes nothing and returns false when source is absent and Keychain lookup fails', function () {
      const { sourceDir, destDir } = makeDirs();
      execSyncStub.throws(new Error('security: item not found'));

      const result = withPlatform('darwin', () =>
        claudeCredentials.provisionClaudeCredentials({ sourceDir, destDir })
      );

      assert.strictEqual(result, false);
      assert.strictEqual(fs.existsSync(path.join(destDir, '.credentials.json')), false);
    });

    it('returns false on non-darwin when no source credentials file exists', function () {
      const { sourceDir, destDir } = makeDirs();

      const result = withPlatform('linux', () =>
        claudeCredentials.provisionClaudeCredentials({ sourceDir, destDir })
      );

      assert.strictEqual(result, false);
      assert.strictEqual(execSyncStub.called, false);
      assert.strictEqual(fs.existsSync(path.join(destDir, '.credentials.json')), false);
    });
  });
});
