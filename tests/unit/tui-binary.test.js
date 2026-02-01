/**
 * Test: TUI Binary helpers
 *
 * Validates platform/arch mapping, asset naming, download URL building,
 * and env overrides used by the install script and launcher.
 */

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const {
  ENV_BINARY_PATH,
  ENV_BINARY_SKIP,
  getAssetName,
  getInstalledBinaryPath,
  resolveBinaryPathOverride,
  resolveDownloadUrl,
  resolveTarget,
  shouldSkipBinaryInstall,
} = require('../../lib/tui-binary');

describe('TUI Binary helpers', function () {
  const originalEnv = { ...process.env };

  afterEach(function () {
    process.env = { ...originalEnv };
  });

  it('builds asset names for supported targets', function () {
    assert.strictEqual(getAssetName('darwin', 'arm64'), 'zeroshot-tui-darwin-arm64.tar.gz');
    assert.strictEqual(getAssetName('linux', 'x64'), 'zeroshot-tui-linux-x64.tar.gz');
  });

  it('returns null for unsupported targets', function () {
    assert.strictEqual(resolveTarget('win32', 'x64'), null);
    assert.strictEqual(resolveTarget('darwin', 'ia32'), null);
  });

  it('builds release URLs with version overrides', function () {
    const url = resolveDownloadUrl({
      version: '1.2.3',
      platform: 'darwin',
      arch: 'arm64',
    });
    assert.strictEqual(
      url,
      'https://github.com/covibes/zeroshot/releases/download/v1.2.3/zeroshot-tui-darwin-arm64.tar.gz'
    );
  });

  it('uses binary path override when provided', function () {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-tui-test-'));
    const tempBin = path.join(tempDir, 'zeroshot-tui');
    fs.writeFileSync(tempBin, 'binary');

    process.env[ENV_BINARY_PATH] = tempBin;

    assert.strictEqual(resolveBinaryPathOverride(), tempBin);
  });

  it('throws when binary override path is missing', function () {
    process.env[ENV_BINARY_PATH] = '/tmp/missing-zeroshot-tui';
    assert.throws(() => resolveBinaryPathOverride(), /Rust TUI binary not found/);
  });

  it('returns installed binary path', function () {
    const expected = path.join(path.resolve(__dirname, '..', '..'), 'libexec', 'zeroshot-tui');
    assert.strictEqual(getInstalledBinaryPath(), expected);
  });

  it('honors skip env values', function () {
    assert.strictEqual(shouldSkipBinaryInstall(), false);

    process.env[ENV_BINARY_SKIP] = '1';
    assert.strictEqual(shouldSkipBinaryInstall(), true);

    process.env[ENV_BINARY_SKIP] = 'false';
    assert.strictEqual(shouldSkipBinaryInstall(), false);
  });
});
