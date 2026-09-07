'use strict';

const childProcess = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const zlib = require('zlib');
const {
  archiveName: artifactArchiveName,
  extractExecutable,
  parseChecksumManifest,
  sha256,
  targetForHost: findTargetForHost,
  verifyChecksum,
} = require('../../npm/zeroshot/lib/release-artifacts');

const repositoryRoot = path.resolve(__dirname, '../..');
const targetManifestPath = path.join(repositoryRoot, 'distribution', 'zeroshot-targets.json');
const targets = Object.freeze(JSON.parse(fs.readFileSync(targetManifestPath, 'utf8')));
const RELEASE_TAG_PREFIX = 'v';
const MINIMUM_RELEASE_MAJOR = 8;

function normalizeVersion(tag) {
  const version =
    typeof tag === 'string' && tag.startsWith(RELEASE_TAG_PREFIX) ? tag.slice(1) : tag;
  const match = typeof version === 'string' ? /^(\d+)\.(\d+)\.(\d+)$/.exec(version) : null;
  if (!match) {
    throw new Error(
      `invalid Zeroshot release version ${JSON.stringify(tag)}; expected X.Y.Z or ${RELEASE_TAG_PREFIX}X.Y.Z`
    );
  }
  if (Number(match[1]) < MINIMUM_RELEASE_MAJOR) {
    throw new Error(
      `invalid Zeroshot release version ${JSON.stringify(tag)}; major version must be ${MINIMUM_RELEASE_MAJOR} or newer`
    );
  }
  return version;
}

function releaseTag(version) {
  return `${RELEASE_TAG_PREFIX}${normalizeVersion(version)}`;
}

function archiveName(version, target) {
  return artifactArchiveName(normalizeVersion(version), target);
}

function targetForHost(platform, arch) {
  return findTargetForHost(targets, platform, arch);
}

function writeOctal(buffer, offset, length, value) {
  const encoded = value.toString(8).padStart(length - 1, '0') + '\0';
  buffer.write(encoded, offset, length, 'ascii');
}

function tarEntry(name, contents, mode = 0o755) {
  if (Buffer.byteLength(name) > 100) throw new Error(`archive entry name is too long: ${name}`);
  const header = Buffer.alloc(512);
  header.write(name, 0, 100, 'utf8');
  writeOctal(header, 100, 8, mode);
  writeOctal(header, 108, 8, 0);
  writeOctal(header, 116, 8, 0);
  writeOctal(header, 124, 12, contents.length);
  writeOctal(header, 136, 12, 0);
  header.fill(0x20, 148, 156);
  header[156] = '0'.charCodeAt(0);
  header.write('ustar\0', 257, 6, 'ascii');
  header.write('00', 263, 2, 'ascii');
  writeOctal(
    header,
    148,
    8,
    [...header].reduce((sum, byte) => sum + byte, 0)
  );
  const padding = Buffer.alloc((512 - (contents.length % 512)) % 512);
  return Buffer.concat([header, contents, padding]);
}

function createArchive(binary, executable) {
  const tar = Buffer.concat([tarEntry(executable, binary), Buffer.alloc(1024)]);
  return zlib.gzipSync(tar, { level: 9, mtime: 0 });
}

function packageTarget({ target, version, binaryPath, outputDirectory }) {
  const declaration = targets.find((candidate) => candidate.target === target);
  if (!declaration) throw new Error(`undeclared Zeroshot release target: ${target}`);
  const binary = fs.readFileSync(binaryPath);
  const filename = archiveName(version, target);
  fs.mkdirSync(outputDirectory, { recursive: true });
  fs.writeFileSync(
    path.join(outputDirectory, filename),
    createArchive(binary, declaration.executable)
  );
  return filename;
}

function createManifest({ version, directory }) {
  const entries = targets.map(({ target }) => {
    const filename = archiveName(version, target);
    const contents = fs.readFileSync(path.join(directory, filename));
    return `${sha256(contents)}  ${filename}`;
  });
  const manifest = `${entries.join('\n')}\n`;
  fs.writeFileSync(path.join(directory, 'SHA256SUMS'), manifest);
  verifyDistribution({ version, directory });
  return manifest;
}

function verifyDistribution({ version, directory }) {
  const expected = targets.map(({ target }) => archiveName(version, target));
  const manifest = parseChecksumManifest(
    fs.readFileSync(path.join(directory, 'SHA256SUMS'), 'utf8')
  );
  if (manifest.size !== expected.length || expected.some((filename) => !manifest.has(filename))) {
    throw new Error(
      `DISTRIBUTION_INCOMPLETE: SHA256SUMS must contain exactly ${expected.length} declared archives`
    );
  }
  for (const declaration of targets) {
    const filename = archiveName(version, declaration.target);
    const archive = fs.readFileSync(path.join(directory, filename));
    verifyChecksum(filename, archive, manifest);
    extractExecutable(archive, declaration.executable);
  }
  return true;
}

function runGh(args) {
  return childProcess.execFileSync('gh', args, { encoding: 'utf8' });
}

function publishAssets({ tag, directory, invokeGh = runGh }) {
  const names = [...targets.map(({ target }) => archiveName(tag, target)), 'SHA256SUMS'];
  const localAssets = new Map(
    names.map((name) => [name, fs.readFileSync(path.join(directory, name))])
  );
  const release = JSON.parse(invokeGh(['release', 'view', tag, '--json', 'assets']));
  const existingNames = release.assets.map(({ name }) => name);
  if (new Set(existingNames).size !== existingNames.length) {
    throw new Error('RELEASE_ASSET_CONFLICT: GitHub Release contains duplicate asset names');
  }
  const unexpected = existingNames.filter((name) => !localAssets.has(name));
  if (unexpected.length > 0) {
    throw new Error(
      `RELEASE_ASSET_CONFLICT: GitHub Release contains unexpected assets: ${unexpected.join(', ')}`
    );
  }

  const existingRequired = names.filter((name) => existingNames.includes(name));
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-assets-'));
  try {
    for (const name of existingRequired) {
      invokeGh(['release', 'download', tag, '--pattern', name, '--dir', temporary]);
      const published = fs.readFileSync(path.join(temporary, name));
      if (!published.equals(localAssets.get(name))) {
        throw new Error(`RELEASE_ASSET_CONFLICT: existing ${name} differs from verified artifact`);
      }
    }
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }

  const missing = names.filter((name) => !existingNames.includes(name));
  for (const name of missing) {
    invokeGh(['release', 'upload', tag, path.join(directory, name)]);
  }
  return { existing: existingRequired, uploaded: missing };
}

module.exports = {
  MINIMUM_RELEASE_MAJOR,
  RELEASE_TAG_PREFIX,
  archiveName,
  createArchive,
  createManifest,
  extractExecutable,
  normalizeVersion,
  packageTarget,
  parseChecksumManifest,
  publishAssets,
  releaseTag,
  repositoryRoot,
  sha256,
  targetForHost,
  targets,
  verifyChecksum,
  verifyDistribution,
};
