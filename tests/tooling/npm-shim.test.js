'use strict';

const assert = require('node:assert/strict');
const { describe, it } = require('node:test');

const distribution = require('../../scripts/distribution');
const installer = require('../../npm/zeroshot/lib/install');
const packageManifest = require('../../npm/zeroshot/package.json');

describe('npm binary shim', () => {
  it('owns the canonical public package and command', () => {
    assert.equal(packageManifest.name, '@the-open-engine-company/zeroshot');
    assert.deepEqual(packageManifest.bin, { zeroshot: 'bin/zeroshot.js' });
    assert.equal(packageManifest.scripts.postinstall, 'node install.js');
  });

  it('downloads from canonical GitHub v-tags', () => {
    assert.equal(installer.RELEASE_TAG_PREFIX, 'v');
    assert.equal(
      installer.archiveName('8.0.0', 'aarch64-apple-darwin'),
      'zeroshot-v8.0.0-aarch64-apple-darwin.tar.gz'
    );
  });

  it('matches the authoritative target manifest', () => {
    for (const target of distribution.targets) {
      assert.deepEqual(installer.selectTarget(target.platform, target.arch), {
        target: target.target,
        executable: target.executable,
      });
    }
    assert.throws(() => installer.selectTarget('plan9', 'mips'), /UNSUPPORTED_ZEROSHOT_HOST/);
  });

  it('verifies archive checksums before extraction', () => {
    const archive = distribution.createArchive(Buffer.from('binary'), 'zeroshot');
    const filename = installer.archiveName('8.0.0', 'x86_64-unknown-linux-musl');
    const manifest = `${distribution.sha256(archive)}  ${filename}\n`;
    assert.doesNotThrow(() => installer.verifyArchive(filename, archive, manifest));
    assert.throws(
      () => installer.verifyArchive(filename, Buffer.from('tampered'), manifest),
      /CHECKSUM_MISMATCH/
    );
  });
});
