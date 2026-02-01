'use strict';
const ENV_BINARY_PATH = 'ZEROSHOT_TUI_BINARY_PATH';
const ENV_BINARY_URL = 'ZEROSHOT_TUI_BINARY_URL';
const ENV_BINARY_SKIP = 'ZEROSHOT_TUI_BINARY_SKIP';

const fs = require('fs');
const path = require('path');

const PACKAGE_ROOT = path.resolve(__dirname, '..');
const BIN_DIR = path.join(PACKAGE_ROOT, 'libexec');
const DEFAULT_RUST_BIN_NAME = process.platform === 'win32' ? 'zeroshot-tui.exe' : 'zeroshot-tui';

const SUPPORTED_PLATFORMS = Object.freeze({
  darwin: 'darwin',
  linux: 'linux',
});

const SUPPORTED_ARCHES = Object.freeze({
  x64: 'x64',
  arm64: 'arm64',
});

function getPackageVersion() {
  // eslint-disable-next-line global-require
  const pkg = require(path.join(PACKAGE_ROOT, 'package.json'));
  return pkg.version;
}

function resolveTargetPlatform(platform = process.platform) {
  return SUPPORTED_PLATFORMS[platform] || null;
}

function resolveTargetArch(arch = process.arch) {
  return SUPPORTED_ARCHES[arch] || null;
}

function resolveTarget(platform = process.platform, arch = process.arch) {
  const resolvedPlatform = resolveTargetPlatform(platform);
  const resolvedArch = resolveTargetArch(arch);
  if (!resolvedPlatform || !resolvedArch) {
    return null;
  }
  return { platform: resolvedPlatform, arch: resolvedArch };
}

function getAssetName(platform, arch) {
  if (!platform || !arch) {
    throw new Error('platform and arch are required to build asset name');
  }
  return `zeroshot-tui-${platform}-${arch}.tar.gz`;
}

function getReleaseBaseUrl(version) {
  if (!version) {
    throw new Error('version is required to build release URL');
  }
  return `https://github.com/covibes/zeroshot/releases/download/v${version}`;
}

function resolveDownloadUrl({ version, platform, arch, overrideUrl } = {}) {
  if (overrideUrl) {
    return overrideUrl;
  }
  const target = resolveTarget(platform, arch);
  if (!target) {
    return null;
  }
  const resolvedVersion = version || getPackageVersion();
  const assetName = getAssetName(target.platform, target.arch);
  return `${getReleaseBaseUrl(resolvedVersion)}/${assetName}`;
}

function getInstallDir() {
  return BIN_DIR;
}

function getInstalledBinaryPath() {
  return path.join(BIN_DIR, DEFAULT_RUST_BIN_NAME);
}

function resolveBinaryPathOverride(env = process.env) {
  const value = env[ENV_BINARY_PATH];
  if (!value) {
    return null;
  }
  const resolved = path.resolve(value);
  if (!fs.existsSync(resolved)) {
    throw new Error(`Rust TUI binary not found at ${resolved}`);
  }
  return resolved;
}

function shouldSkipBinaryInstall(env = process.env) {
  const raw = env[ENV_BINARY_SKIP];
  if (!raw) {
    return false;
  }
  const normalized = String(raw).trim().toLowerCase();
  return !['0', 'false', 'no', 'off'].includes(normalized);
}

module.exports = {
  BIN_DIR,
  DEFAULT_RUST_BIN_NAME,
  ENV_BINARY_PATH,
  ENV_BINARY_URL,
  ENV_BINARY_SKIP,
  SUPPORTED_PLATFORMS,
  SUPPORTED_ARCHES,
  getAssetName,
  getInstallDir,
  getInstalledBinaryPath,
  getPackageVersion,
  getReleaseBaseUrl,
  resolveBinaryPathOverride,
  resolveDownloadUrl,
  resolveTarget,
  resolveTargetArch,
  resolveTargetPlatform,
  shouldSkipBinaryInstall,
};
