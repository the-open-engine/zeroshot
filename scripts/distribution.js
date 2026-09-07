#!/usr/bin/env node
'use strict';

const childProcess = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const artifacts = require('./distribution/artifacts');
const { checkRepository } = require('./distribution/repository');
const versioning = require('./distribution/version');

const COMMANDS = Object.freeze([
  'package',
  'manifest',
  'verify',
  'dry-run',
  'stage-version',
  'check-version',
  'check-repository',
  'print-version',
  'smoke',
  'smoke-archive',
  'publish-assets',
]);

function argument(name) {
  const index = process.argv.indexOf(`--${name}`);
  if (index === -1 || !process.argv[index + 1]) throw new Error(`missing --${name}`);
  return process.argv[index + 1];
}

function runPackageCommand() {
  const filename = artifacts.packageTarget({
    target: argument('target'),
    version: argument('version'),
    binaryPath: argument('binary'),
    outputDirectory: argument('out'),
  });
  process.stdout.write(`${filename}\n`);
}

function runManifestCommand() {
  artifacts.createManifest({ version: argument('version'), directory: argument('dir') });
  process.stdout.write(`verified ${artifacts.targets.length} archives and SHA256SUMS\n`);
}

function runVerifyCommand() {
  artifacts.verifyDistribution({ version: argument('version'), directory: argument('dir') });
  process.stdout.write(`verified ${artifacts.targets.length} existing archives and SHA256SUMS\n`);
}

function runDryRunCommand() {
  const version = argument('version');
  const binaryPath = argument('binary');
  const outputDirectory = argument('out');
  for (const { target } of artifacts.targets) {
    artifacts.packageTarget({ target, version, binaryPath, outputDirectory });
  }
  artifacts.createManifest({ version, directory: outputDirectory });
  process.stdout.write(`dry-run produced and verified ${artifacts.targets.length} archives\n`);
}

function runStageVersionCommand() {
  const staged = versioning.stageVersion(argument('tag'));
  process.stdout.write(
    `staged Zeroshot package version ${staged.currentVersion} -> ${staged.version}\n`
  );
}

function runCheckVersionCommand() {
  const version = versioning.checkVersionCoupling(argument('tag'));
  process.stdout.write(`Zeroshot package version matches release tag: ${version}\n`);
}

function runPrintVersionCommand() {
  const manifest = fs.readFileSync(
    path.join(artifacts.repositoryRoot, 'zeroshot', 'Cargo.toml'),
    'utf8'
  );
  process.stdout.write(`${versioning.cargoVersion(manifest)}\n`);
}

function smokeExecutable(
  binaryPath,
  failureCode,
  expectedVersion = versioning.cargoVersion(
    fs.readFileSync(path.join(artifacts.repositoryRoot, 'zeroshot', 'Cargo.toml'), 'utf8')
  )
) {
  const result = childProcess.spawnSync(binaryPath, ['--version'], { encoding: 'utf8' });
  if (result.error) throw result.error;
  if (result.signal || result.status !== 0) {
    throw new Error(`${failureCode}: status=${result.status} signal=${result.signal || 'none'}`);
  }
  const expected = `zeroshot ${expectedVersion}\n`;
  if (result.stdout !== expected) {
    throw new Error(
      `${failureCode}: expected ${JSON.stringify(expected.trim())}, received ${JSON.stringify(result.stdout.trim())}`
    );
  }
}

function runSmokeCommand() {
  const binaryPath = path.resolve(argument('binary'));
  smokeExecutable(binaryPath, 'ZEROSHOT_BINARY_SMOKE_FAILED');
  process.stdout.write(`Zeroshot release executable exited 0: ${binaryPath}\n`);
}

function runSmokeArchiveCommand() {
  const target = argument('target');
  const declaration = artifacts.targets.find((candidate) => candidate.target === target);
  if (!declaration) throw new Error(`undeclared Zeroshot release target: ${target}`);
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-smoke-'));
  const binaryPath = path.join(directory, declaration.executable);
  try {
    const archive = fs.readFileSync(argument('archive'));
    fs.writeFileSync(binaryPath, artifacts.extractExecutable(archive, declaration.executable), {
      mode: 0o755,
    });
    smokeExecutable(binaryPath, 'ZEROSHOT_ARCHIVE_SMOKE_FAILED');
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
  process.stdout.write(`Zeroshot release archive executable exited 0: ${target}\n`);
}

function runPublishAssetsCommand() {
  const result = artifacts.publishAssets({ tag: argument('tag'), directory: argument('dir') });
  process.stdout.write(
    `verified ${result.existing.length} existing assets and ` +
      `uploaded ${result.uploaded.length} missing assets\n`
  );
}

function runCheckRepositoryCommand() {
  checkRepository();
  process.stdout.write(
    `Zeroshot distribution declares ${artifacts.targets.length} complete targets\n`
  );
}

const commandHandlers = new Map([
  ['package', runPackageCommand],
  ['manifest', runManifestCommand],
  ['verify', runVerifyCommand],
  ['dry-run', runDryRunCommand],
  ['stage-version', runStageVersionCommand],
  ['check-version', runCheckVersionCommand],
  ['print-version', runPrintVersionCommand],
  ['smoke', runSmokeCommand],
  ['smoke-archive', runSmokeArchiveCommand],
  ['publish-assets', runPublishAssetsCommand],
  ['check-repository', runCheckRepositoryCommand],
]);

function run() {
  const handler = commandHandlers.get(process.argv[2]);
  if (!handler) throw new Error(`usage: distribution.js <${COMMANDS.join('|')}>`);
  handler();
}

if (require.main === module) {
  try {
    run();
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}

module.exports = {
  MINIMUM_RELEASE_MAJOR: artifacts.MINIMUM_RELEASE_MAJOR,
  RELEASE_TAG_PREFIX: artifacts.RELEASE_TAG_PREFIX,
  VERSION_ERROR: versioning.VERSION_ERROR,
  archiveName: artifacts.archiveName,
  cargoVersion: versioning.cargoVersion,
  checkRepository,
  checkVersionCoupling: versioning.checkVersionCoupling,
  createArchive: artifacts.createArchive,
  createManifest: artifacts.createManifest,
  extractExecutable: artifacts.extractExecutable,
  normalizeVersion: artifacts.normalizeVersion,
  packageTarget: artifacts.packageTarget,
  parseChecksumManifest: artifacts.parseChecksumManifest,
  publishAssets: artifacts.publishAssets,
  releaseTag: artifacts.releaseTag,
  sha256: artifacts.sha256,
  smokeExecutable,
  stageVersion: versioning.stageVersion,
  targetForHost: artifacts.targetForHost,
  targets: artifacts.targets,
  verifyChecksum: artifacts.verifyChecksum,
  verifyDistribution: artifacts.verifyDistribution,
};
