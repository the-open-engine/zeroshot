'use strict';

const crypto = require('crypto');
const { execFile } = require('child_process');
const fs = require('fs');
const http = require('http');
const https = require('https');
const os = require('os');
const path = require('path');
const { pipeline } = require('stream/promises');
const { URL } = require('url');
const { promisify } = require('util');

const {
  ENV_BINARY_PATH,
  ENV_BINARY_URL,
  ENV_BINARY_SKIP,
  getAssetName,
  getInstallDir,
  getInstalledBinaryPath,
  resolveBinaryPathOverride,
  resolveDownloadUrl,
  resolveTarget,
  shouldSkipBinaryInstall,
} = require('../lib/tui-binary');

const MAX_REDIRECTS = 5;
const execFileAsync = promisify(execFile);
const PACKAGE_ROOT = path.resolve(__dirname, '..');

function isGitCheckout() {
  let current = PACKAGE_ROOT;
  while (true) {
    const gitPath = path.join(current, '.git');
    if (fs.existsSync(gitPath)) {
      return true;
    }
    const parent = path.dirname(current);
    if (parent === current) {
      return false;
    }
    current = parent;
  }
}

function isNotFoundError(error) {
  if (!error || typeof error.message !== 'string') {
    return false;
  }
  return /\(404\)/.test(error.message);
}

async function main() {
  if (shouldSkipBinaryInstall()) {
    console.log(`${ENV_BINARY_SKIP} set; skipping Rust TUI binary install.`);
    return;
  }

  const installContext = await prepareInstallContext();
  if (installContext.overridePath) {
    console.log(
      `${ENV_BINARY_PATH} set; using local Rust TUI binary at ${installContext.overridePath}.`
    );
    await installFromLocalBinary(installContext.overridePath, installContext.installPath);
    return;
  }

  const downloadContext = resolveDownloadContext();
  if (!downloadContext) {
    console.log(
      `Rust TUI binary not supported on ${process.platform}/${process.arch}; skipping install.`
    );
    return;
  }

  await downloadAndInstall(downloadContext, installContext);
}

async function prepareInstallContext() {
  const installDir = getInstallDir();
  const installPath = getInstalledBinaryPath();
  await fs.promises.mkdir(installDir, { recursive: true });
  const overridePath = resolveBinaryPathOverride();
  return { installDir, installPath, overridePath };
}

function resolveDownloadContext() {
  const overrideUrl = process.env[ENV_BINARY_URL];
  const target = overrideUrl ? null : resolveTarget();
  if (!overrideUrl && !target) {
    return null;
  }

  const archiveUrl = resolveDownloadUrl({
    platform: target?.platform,
    arch: target?.arch,
    overrideUrl,
  });

  if (!archiveUrl) {
    throw new Error('Unable to resolve Rust TUI binary download URL.');
  }

  const assetName = target
    ? getAssetName(target.platform, target.arch)
    : resolveAssetNameFromUrl(archiveUrl);

  return { archiveUrl, assetName };
}

async function downloadAndInstall(downloadContext, installContext) {
  const tempDir = await fs.promises.mkdtemp(path.join(os.tmpdir(), 'zeroshot-tui-'));
  const archivePath = path.join(tempDir, downloadContext.assetName);

  try {
    try {
      await downloadToFile(downloadContext.archiveUrl, archivePath, MAX_REDIRECTS);
    } catch (error) {
      if (isGitCheckout() && isNotFoundError(error)) {
        console.log(
          'Rust TUI binary not found for this version in git checkout; skipping install. ' +
            `Set ${ENV_BINARY_URL} or ${ENV_BINARY_PATH} to override.`
        );
        return;
      }
      throw error;
    }
    const shaUrl = `${downloadContext.archiveUrl}.sha256`;
    const expectedSha = await fetchSha256(shaUrl);
    if (expectedSha) {
      await verifySha256(archivePath, expectedSha);
    }

    await extractArchive(archivePath, installContext.installDir);
    await ensureBinaryInstalled(installContext.installPath, installContext.installDir);

    await fs.promises.chmod(installContext.installPath, 0o755);
    console.log(`Installed Rust TUI binary to ${installContext.installPath}`);
  } finally {
    await fs.promises.rm(tempDir, { recursive: true, force: true });
  }
}

function resolveAssetNameFromUrl(url) {
  try {
    const parsed = new URL(url);
    const base = path.basename(parsed.pathname);
    return base || 'zeroshot-tui.tar.gz';
  } catch {
    return 'zeroshot-tui.tar.gz';
  }
}

async function installFromLocalBinary(sourcePath, destinationPath) {
  const stats = await fs.promises.stat(sourcePath).catch(() => null);
  if (!stats || !stats.isFile()) {
    throw new Error(`Rust TUI binary not found at ${sourcePath}`);
  }
  await fs.promises.copyFile(sourcePath, destinationPath);
  await fs.promises.chmod(destinationPath, 0o755);
  console.log(`Installed Rust TUI binary from ${sourcePath}`);
}

async function downloadToFile(url, destination, redirectsRemaining) {
  const client = getHttpClient(url);
  await new Promise((resolve, reject) => {
    const request = client.get(
      url,
      { headers: { 'User-Agent': '@covibes/zeroshot' } },
      (response) => {
        handleDownloadResponse({
          response,
          url,
          destination,
          redirectsRemaining,
        })
          .then(resolve)
          .catch(reject);
      }
    );

    request.on('error', reject);
  });
}

function getHttpClient(url) {
  return url.startsWith('https://') ? https : http;
}

function isRedirectStatus(status) {
  return [301, 302, 303, 307, 308].includes(status);
}

function resolveRedirectUrl(baseUrl, location) {
  try {
    return new URL(location, baseUrl).toString();
  } catch {
    return location;
  }
}

async function handleDownloadResponse({ response, url, destination, redirectsRemaining }) {
  const status = response.statusCode || 0;
  if (isRedirectStatus(status)) {
    await followRedirect(response, url, destination, redirectsRemaining);
    return;
  }

  if (status !== 200) {
    response.resume();
    throw new Error(`Download failed (${status}) for ${url}`);
  }

  await streamToFileWithLengthCheck(response, destination, url);
}

async function followRedirect(response, url, destination, redirectsRemaining) {
  if (redirectsRemaining <= 0) {
    response.resume();
    throw new Error(`Too many redirects while downloading ${url}`);
  }
  const location = response.headers.location;
  if (!location) {
    response.resume();
    throw new Error(`Redirect without location while downloading ${url}`);
  }
  const nextUrl = resolveRedirectUrl(url, location);
  response.resume();
  await downloadToFile(nextUrl, destination, redirectsRemaining - 1);
}

async function streamToFileWithLengthCheck(response, destination, url) {
  const expectedLength = parseInt(response.headers['content-length'], 10);
  let downloaded = 0;
  const fileStream = fs.createWriteStream(destination);

  response.on('data', (chunk) => {
    downloaded += chunk.length;
  });

  await pipeline(response, fileStream);

  if (!Number.isNaN(expectedLength) && expectedLength > 0 && downloaded !== expectedLength) {
    throw new Error(
      `Download size mismatch for ${url}: expected ${expectedLength}, got ${downloaded}`
    );
  }
}

function fetchSha256(url, redirectsRemaining = MAX_REDIRECTS) {
  const client = getHttpClient(url);

  return new Promise((resolve, reject) => {
    const request = client.get(
      url,
      { headers: { 'User-Agent': '@covibes/zeroshot' } },
      (response) => {
        const status = response.statusCode || 0;
        if (isRedirectStatus(status)) {
          if (redirectsRemaining <= 0) {
            response.resume();
            reject(new Error(`Too many redirects while downloading ${url}`));
            return;
          }
          const location = response.headers.location;
          if (!location) {
            response.resume();
            reject(new Error(`Redirect without location while downloading ${url}`));
            return;
          }
          const nextUrl = resolveRedirectUrl(url, location);
          response.resume();
          resolve(fetchSha256(nextUrl, redirectsRemaining - 1));
          return;
        }
        if (status === 404) {
          response.resume();
          resolve(null);
          return;
        }
        if (status !== 200) {
          response.resume();
          reject(new Error(`Checksum download failed (${status}) for ${url}`));
          return;
        }
        let body = '';
        response.setEncoding('utf8');
        response.on('data', (chunk) => {
          body += chunk;
        });
        response.on('end', () => {
          const match = body.trim().match(/^([a-fA-F0-9]{64})/);
          if (!match) {
            reject(new Error(`Invalid sha256 contents from ${url}`));
            return;
          }
          resolve(match[1].toLowerCase());
        });
      }
    );

    request.on('error', reject);
  });
}

async function verifySha256(filePath, expected) {
  const hash = crypto.createHash('sha256');
  const stream = fs.createReadStream(filePath);
  await new Promise((resolve, reject) => {
    stream.on('error', reject);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.on('end', resolve);
  });
  const actual = hash.digest('hex');
  if (actual !== expected) {
    throw new Error(`Checksum mismatch for ${filePath}: expected ${expected}, got ${actual}`);
  }
}

async function extractArchive(archivePath, destinationDir) {
  const entries = await listArchiveEntries(archivePath);
  const unsafeEntry = entries.find((entry) => !isSafeTarEntry(entry));
  if (unsafeEntry) {
    throw new Error(`Unsafe archive entry detected: ${unsafeEntry}`);
  }
  await execFileAsync('tar', ['-xzf', archivePath, '-C', destinationDir]);
}

async function listArchiveEntries(archivePath) {
  const { stdout } = await execFileAsync('tar', ['-tf', archivePath]);
  return stdout
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);
}

function isSafeTarEntry(entry) {
  if (!entry || entry.includes('\0')) {
    return false;
  }
  const normalized = path.posix.normalize(entry);
  if (path.posix.isAbsolute(normalized)) {
    return false;
  }
  if (normalized === '..' || normalized.startsWith('../')) {
    return false;
  }
  return true;
}

async function ensureBinaryInstalled(installPath, installDir) {
  if (fs.existsSync(installPath)) {
    return;
  }
  const defaultBinary = path.join(installDir, 'zeroshot-tui');
  if (fs.existsSync(defaultBinary)) {
    await fs.promises.rename(defaultBinary, installPath);
    return;
  }
  throw new Error(`Expected Rust TUI binary at ${installPath} after extraction`);
}

main().catch((error) => {
  console.error('Failed to install Rust TUI binary');
  console.error(error.stack || error.message);
  process.exit(1);
});
