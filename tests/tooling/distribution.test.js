'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { describe, it } = require('node:test');

const distribution = require('../../scripts/distribution');

function writeReleaseAssets(directory, version = 'v8.0.0') {
  for (const target of distribution.targets) {
    fs.writeFileSync(
      path.join(directory, distribution.archiveName(version, target.target)),
      'fixture'
    );
  }
  fs.writeFileSync(path.join(directory, 'SHA256SUMS'), 'fixture');
  fs.writeFileSync(path.join(directory, distribution.releaseNotesName(version)), 'notes fixture');
}

describe('canonical distribution', () => {
  it('uses canonical v8+ release tags', () => {
    assert.equal(distribution.normalizeVersion('8.0.0'), '8.0.0');
    assert.equal(distribution.normalizeVersion('v8.1.2'), '8.1.2');
    assert.equal(distribution.releaseTag('8.1.2'), 'v8.1.2');
    assert.throws(() => distribution.normalizeVersion('7.0.1'), /major version must be 8 or newer/);
    assert.throws(() => distribution.normalizeVersion('zeroshot-v8.0.0'), /expected X\.Y\.Z/);
  });

  it('builds deterministic single-executable archives', () => {
    const binary = Buffer.from('zeroshot-binary');
    const first = distribution.createArchive(binary, 'zeroshot');
    const second = distribution.createArchive(binary, 'zeroshot');
    assert.deepEqual(first, second);
    assert.deepEqual(distribution.extractExecutable(first, 'zeroshot'), binary);
    assert.equal(
      distribution.archiveName('v8.0.0', 'x86_64-unknown-linux-musl'),
      'zeroshot-v8.0.0-x86_64-unknown-linux-musl.tar.gz'
    );
    assert.equal(distribution.releaseNotesName('v8.0.0'), 'zeroshot-release-notes-v8.0.0.md');
  });

  it('creates and verifies the complete declared target set', () => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-distribution-test-'));
    const binary = path.join(directory, 'zeroshot');
    fs.writeFileSync(binary, 'fixture');
    try {
      for (const target of distribution.targets) {
        distribution.packageTarget({
          target: target.target,
          version: '8.0.0',
          binaryPath: binary,
          outputDirectory: directory,
        });
      }
      distribution.createManifest({ version: '8.0.0', directory });
      assert.equal(distribution.verifyDistribution({ version: 'v8.0.0', directory }), true);
    } finally {
      fs.rmSync(directory, { recursive: true, force: true });
    }
  });

  it('validates the repository release contract', () => {
    assert.equal(distribution.checkRepository(), true);
  });

  it('rejects releases that omit the embedded UI build', () => {
    const workflow = fs.readFileSync(
      path.join(__dirname, '..', '..', '.github', 'workflows', 'release.yml'),
      'utf8'
    );
    for (const required of [
      'npm --prefix ui ci --ignore-scripts',
      'npm --prefix ui run build',
      '--features ui',
    ]) {
      assert.throws(
        () => distribution.checkRepository({ workflow: workflow.replace(required, '') }),
        /ZEROSHOT_DISTRIBUTION_INTEGRITY/,
        required
      );
    }
  });
});

describe('release asset publication', () => {
  it('rejects unexpected assets on an existing GitHub Release', () => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-release-assets-'));
    try {
      writeReleaseAssets(directory);
      assert.throws(
        () =>
          distribution.publishAssets({
            tag: 'v8.0.0',
            directory,
            invokeGh: () => JSON.stringify({ assets: [{ name: 'zeroshot-rust-v7.tar.gz' }] }),
          }),
        /unexpected assets: zeroshot-rust-v7\.tar\.gz/
      );
    } finally {
      fs.rmSync(directory, { recursive: true, force: true });
    }
  });

  it('rejects a changed immutable release-note asset', () => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-release-assets-'));
    const notesName = distribution.releaseNotesName('v8.0.0');
    try {
      writeReleaseAssets(directory);
      fs.writeFileSync(path.join(directory, notesName), 'expected notes');
      assert.throws(
        () =>
          distribution.publishAssets({
            tag: 'v8.0.0',
            directory,
            invokeGh: (arguments_) => {
              if (arguments_[1] === 'view') {
                return JSON.stringify({ assets: [{ name: notesName }] });
              }
              if (arguments_[1] === 'download') {
                const output = arguments_[arguments_.indexOf('--dir') + 1];
                fs.writeFileSync(path.join(output, notesName), 'changed notes');
                return '';
              }
              return '';
            },
          }),
        new RegExp(`existing ${notesName.replaceAll('.', '\\.')} differs`)
      );
    } finally {
      fs.rmSync(directory, { recursive: true, force: true });
    }
  });
});
