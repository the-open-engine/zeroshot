'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { describe, it } = require('node:test');

const distribution = require('../../scripts/distribution');
const installer = require('../../npm/zeroshot/lib/install');
const packageManifest = require('../../npm/zeroshot/package.json');

const packageSource = path.resolve(__dirname, '..', '..', 'npm', 'zeroshot');

function stageLauncherWithNative(t, nativeScript) {
  const packageRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-npm-shim-'));
  t.after(() => fs.rmSync(packageRoot, { recursive: true, force: true }));
  for (const entry of ['bin/zeroshot.js', 'lib', 'targets.json']) {
    fs.cpSync(path.join(packageSource, entry), path.join(packageRoot, entry), {
      recursive: true,
    });
  }
  const native = path.join(packageRoot, 'bin', 'native', installer.selectTarget().executable);
  fs.mkdirSync(path.dirname(native));
  fs.writeFileSync(native, `#!/bin/sh\n${nativeScript}\n`, { mode: 0o755 });
  return { launcher: path.join(packageRoot, 'bin', 'zeroshot.js'), native };
}

describe('npm binary shim', () => {
  // Node ignores SIGPIPE and SIGXFSZ, so re-raising them cannot end the launcher.
  for (const signal of ['SIGPIPE', 'SIGXFSZ', 'SIGTERM']) {
    it(
      `reports a native process killed by ${signal} as a failure`,
      { skip: process.platform === 'win32' && 'POSIX signals only' },
      (t) => {
        const { launcher, native } = stageLauncherWithNative(
          t,
          `ulimit -c 0\nkill -s ${signal.slice(3)} $$`
        );
        assert.equal(spawnSync(native).signal, signal);

        const result = spawnSync(process.execPath, [launcher], { encoding: 'utf8' });
        assert.equal(result.error, undefined);
        assert.equal(result.stderr, '');
        // A shell reports death by a signal as 128 + its number, so both outcomes look the same.
        const expected = 128 + os.constants.signals[signal];
        assert.equal(
          result.signal === signal ? expected : result.status,
          expected,
          `launcher exited with status ${result.status} and signal ${result.signal}`
        );
      }
    );
  }

  it('owns the canonical public package and command', () => {
    assert.equal(packageManifest.name, '@the-open-engine-company/zeroshot');
    assert.equal(packageManifest.homepage, 'https://zeroshot.sh');
    assert.deepEqual(packageManifest.bin, { zeroshot: 'bin/zeroshot.js' });
    assert.equal(packageManifest.scripts.postinstall, 'node install.js');
    assert.equal(packageManifest.files.includes('skills/'), true);
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
        resticExecutable: target.resticExecutable,
      });
    }
    assert.throws(() => installer.selectTarget('plan9', 'mips'), /UNSUPPORTED_ZEROSHOT_HOST/);
  });

  it('verifies archive checksums before extraction', () => {
    const archive = distribution.createArchive([
      { name: 'zeroshot', contents: Buffer.from('binary') },
      { name: 'restic', contents: Buffer.from('restic') },
    ]);
    const filename = installer.archiveName('8.0.0', 'x86_64-unknown-linux-musl');
    const manifest = `${distribution.sha256(archive)}  ${filename}\n`;
    assert.doesNotThrow(() => installer.verifyArchive(filename, archive, manifest));
    assert.throws(
      () => installer.verifyArchive(filename, Buffer.from('tampered'), manifest),
      /CHECKSUM_MISMATCH/
    );
  });
});
