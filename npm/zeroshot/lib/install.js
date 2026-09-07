'use strict';

const fs = require('fs');
const https = require('https');
const path = require('path');
const { URL } = require('url');
const {
  archiveName,
  extractExecutable,
  parseChecksumManifest,
  targetForHost,
  verifyArchive,
} = require('./release-artifacts');

const RELEASE_BASE_URL = 'https://github.com/the-open-engine/zeroshot/releases/download';
const RELEASE_TAG_PREFIX = 'v';
const MAX_MANIFEST_BYTES = 1024 * 1024;
const MAX_ARCHIVE_BYTES = 256 * 1024 * 1024;
const TARGETS = Object.freeze(
  require('../targets.json').map((declaration) => Object.freeze({ ...declaration }))
);
const HOST_TARGETS = Object.freeze(
  Object.fromEntries(TARGETS.map((target) => [`${target.platform}/${target.arch}`, target]))
);

function selectTarget(platform = process.platform, arch = process.arch) {
  const { target, executable } = targetForHost(TARGETS, platform, arch);
  return { target, executable };
}

function download(url, maximumBytes, redirects = 0) {
  return new Promise((resolve, reject) => {
    const request = https.get(
      url,
      { headers: { 'user-agent': '@the-open-engine-company/zeroshot' } },
      (response) => {
        if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
          response.resume();
          if (redirects >= 5)
            return reject(new Error(`DOWNLOAD_FAILED: too many redirects for ${url}`));
          return download(
            new URL(response.headers.location, url).toString(),
            maximumBytes,
            redirects + 1
          ).then(resolve, reject);
        }
        if (response.statusCode !== 200) {
          response.resume();
          return reject(new Error(`DOWNLOAD_FAILED: ${url} returned HTTP ${response.statusCode}`));
        }
        const chunks = [];
        let length = 0;
        response.on('data', (chunk) => {
          length += chunk.length;
          if (length > maximumBytes)
            request.destroy(new Error(`DOWNLOAD_FAILED: ${url} exceeds ${maximumBytes} bytes`));
          else chunks.push(chunk);
        });
        response.on('end', () => resolve(Buffer.concat(chunks)));
      }
    );
    request.on('error', reject);
  });
}

function isReleaseVersion(version) {
  if (typeof version !== 'string') return false;
  const prereleaseStart = version.indexOf('-');
  const core = prereleaseStart === -1 ? version : version.slice(0, prereleaseStart);
  const prerelease = prereleaseStart === -1 ? '' : version.slice(prereleaseStart + 1);
  const coreParts = core.split('.');
  if (coreParts.length !== 3 || coreParts.some((part) => !part || !/^\d+$/.test(part))) {
    return false;
  }
  return (
    !prerelease ||
    prerelease
      .split('.')
      .every(
        (part) =>
          part &&
          [...part].every(
            (character) =>
              (character >= '0' && character <= '9') ||
              (character >= 'A' && character <= 'Z') ||
              (character >= 'a' && character <= 'z') ||
              character === '-'
          )
      )
  );
}

async function install(options = {}) {
  const packageRoot = options.packageRoot || path.resolve(__dirname, '..');
  const metadata =
    options.packageMetadata ||
    JSON.parse(fs.readFileSync(path.join(packageRoot, 'package.json'), 'utf8'));
  if (!isReleaseVersion(metadata.version) || metadata.version === '0.0.0-development') {
    throw new Error(
      `UNRELEASED_SHIM_VERSION: cannot install binary for package version ${metadata.version}`
    );
  }
  const selected = selectTarget(options.platform, options.arch);
  const filename = archiveName(metadata.version, selected.target);
  const baseUrl = `${RELEASE_BASE_URL}/${RELEASE_TAG_PREFIX}${metadata.version}`;
  const fetchBuffer = options.fetchBuffer || download;
  const manifest = await fetchBuffer(`${baseUrl}/SHA256SUMS`, MAX_MANIFEST_BYTES);
  const archive = await fetchBuffer(`${baseUrl}/${filename}`, MAX_ARCHIVE_BYTES);
  verifyArchive(filename, archive, manifest.toString('utf8'));
  const executable = extractExecutable(archive, selected.executable);

  const nativeDirectory = path.join(packageRoot, 'bin', 'native');
  const destination = path.join(nativeDirectory, selected.executable);
  const temporary = `${destination}.${process.pid}.tmp`;
  fs.mkdirSync(nativeDirectory, { recursive: true });
  try {
    fs.writeFileSync(temporary, executable, { mode: 0o755, flag: 'wx' });
    fs.renameSync(temporary, destination);
    if (process.platform !== 'win32') fs.chmodSync(destination, 0o755);
  } finally {
    fs.rmSync(temporary, { force: true });
  }
  return destination;
}

module.exports = {
  HOST_TARGETS,
  RELEASE_BASE_URL,
  RELEASE_TAG_PREFIX,
  archiveName,
  extractExecutable,
  install,
  parseChecksumManifest,
  selectTarget,
  verifyArchive,
};
