'use strict';

const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');
const { normalizeProviderName } = require('./provider-names');
const { getInstalledBinaryPath, resolveBinaryPathOverride } = require('./tui-binary');
const { getProvider } = require('../src/providers');

const PACKAGE_ROOT = path.resolve(__dirname, '..');
const TUI_FALLBACK_ENV = 'ZEROSHOT_TUI';
const TUI_BINARY_ENV = 'ZEROSHOT_TUI_PATH';
const TUI_BINARY_ENV_ALT = 'ZEROSHOT_TUI_BIN';
const TUI_INITIAL_SCREEN_ENV = 'ZEROSHOT_TUI_INITIAL_SCREEN';
const TUI_PROVIDER_OVERRIDE_ENV = 'ZEROSHOT_TUI_PROVIDER_OVERRIDE';
const DEFAULT_RUST_BIN_NAME = process.platform === 'win32' ? 'zeroshot-tui.exe' : 'zeroshot-tui';
const VALID_INITIAL_SCREENS = new Set(['launcher', 'monitor']);

function shouldUseInk() {
  return process.env[TUI_FALLBACK_ENV] === 'ink';
}

function resolveTuiProviderOverride(options = {}) {
  const override = options.providerOverride ?? options.provider;
  if (!override || (typeof override === 'string' && !override.trim())) {
    return null;
  }
  const normalized = normalizeProviderName(override);
  getProvider(normalized);
  return normalized;
}

function resolveInitialScreen(options = {}) {
  const initial = options.initialScreen ?? options.initialView;
  if (!initial || (typeof initial === 'string' && !initial.trim())) {
    return null;
  }
  const normalized = String(initial).trim().toLowerCase();
  if (!VALID_INITIAL_SCREENS.has(normalized)) {
    throw new Error(
      `Unknown initial screen: ${normalized}. Valid: ${[...VALID_INITIAL_SCREENS].join(', ')}`
    );
  }
  return normalized;
}

function resolveRustTuiBinary() {
  const override = resolveBinaryPathOverride();
  if (override) {
    return override;
  }

  const explicit = process.env[TUI_BINARY_ENV] || process.env[TUI_BINARY_ENV_ALT];
  if (explicit) {
    const resolved = path.resolve(explicit);
    if (!fs.existsSync(resolved)) {
      throw new Error(`Rust TUI binary not found at ${resolved}`);
    }
    return resolved;
  }

  const installed = getInstalledBinaryPath();
  if (fs.existsSync(installed)) {
    return installed;
  }

  const candidates = [
    path.join(PACKAGE_ROOT, 'tui-rs', 'target', 'release', DEFAULT_RUST_BIN_NAME),
    path.join(PACKAGE_ROOT, 'tui-rs', 'target', 'debug', DEFAULT_RUST_BIN_NAME),
  ];

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }

  return DEFAULT_RUST_BIN_NAME;
}

function buildRustTuiCommand(options = {}) {
  const initialScreen = resolveInitialScreen(options);
  const providerOverride = resolveTuiProviderOverride(options);
  const command = options.binaryPath || resolveRustTuiBinary();
  const args = [];

  if (initialScreen) {
    args.push('--initial-screen', initialScreen);
  }

  if (providerOverride) {
    args.push('--provider-override', providerOverride);
  }

  const env = { ...process.env };
  if (initialScreen) {
    env[TUI_INITIAL_SCREEN_ENV] = initialScreen;
  }
  if (providerOverride) {
    env[TUI_PROVIDER_OVERRIDE_ENV] = providerOverride;
  }

  return {
    command,
    args,
    env,
    cwd: options.cwd || process.cwd(),
  };
}

function launchRustTui(options = {}) {
  const { command, args, env, cwd } = buildRustTuiCommand(options);
  const spawnFn = options.spawn || spawn;
  const child = spawnFn(command, args, { stdio: 'inherit', env, cwd });

  if (child && typeof child.on === 'function') {
    child.on('error', (error) => {
      console.error(`Failed to start Rust TUI (${command}): ${error.message}`);
      console.error(
        `Set ${TUI_FALLBACK_ENV}=ink for the legacy Ink UI, ${TUI_BINARY_ENV} to a valid binary, or ZEROSHOT_TUI_BINARY_PATH to the installed Rust TUI binary.`
      );
      process.exitCode = 1;
    });
  }

  return child;
}

function buildInkStartOptions(options = {}) {
  return {
    autoExit: false,
    providerOverride: resolveTuiProviderOverride(options),
    initialView: options.initialView,
  };
}

function launchInkTui(options = {}) {
  const startInk = options.startInk || require('./tui').start;
  startInk(buildInkStartOptions(options));
}

function launchTuiSession(options = {}) {
  if (shouldUseInk()) {
    return launchInkTui(options);
  }

  return launchRustTui(options);
}

module.exports = {
  buildInkStartOptions,
  buildRustTuiCommand,
  launchInkTui,
  launchRustTui,
  launchTuiSession,
  resolveInitialScreen,
  resolveRustTuiBinary,
  resolveTuiProviderOverride,
  shouldUseInk,
};
