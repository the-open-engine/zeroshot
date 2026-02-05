'use strict';
const TUI_BINARY_ENV = 'ZEROSHOT_TUI_PATH';
const TUI_BINARY_ENV_ALT = 'ZEROSHOT_TUI_BIN';
const TUI_INITIAL_SCREEN_ENV = 'ZEROSHOT_TUI_INITIAL_SCREEN';
const TUI_PROVIDER_OVERRIDE_ENV = 'ZEROSHOT_TUI_PROVIDER_OVERRIDE';
const TUI_UI_VARIANT_ENV = 'ZEROSHOT_TUI_UI';

const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');
const { normalizeProviderName } = require('./provider-names');
const { getInstalledBinaryPath, resolveBinaryPathOverride } = require('./tui-binary');
const { getProvider } = require('../src/providers');

const PACKAGE_ROOT = path.resolve(__dirname, '..');
const DEFAULT_RUST_BIN_NAME = process.platform === 'win32' ? 'zeroshot-tui.exe' : 'zeroshot-tui';
const VALID_INITIAL_SCREENS = new Set(['launcher', 'monitor']);
const VALID_UI_VARIANTS = new Set(['classic', 'disruptive']);

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

function resolveUiVariant(options = {}) {
  const ui = options.ui;
  if (!ui || (typeof ui === 'string' && !ui.trim())) {
    return null;
  }
  const normalized = String(ui).trim().toLowerCase();
  if (!VALID_UI_VARIANTS.has(normalized)) {
    throw new Error(
      `Unknown UI variant: ${normalized}. Valid: ${[...VALID_UI_VARIANTS].join(', ')}`
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

  const candidates = [
    path.join(PACKAGE_ROOT, 'tui-rs', 'target', 'debug', DEFAULT_RUST_BIN_NAME),
    path.join(PACKAGE_ROOT, 'tui-rs', 'target', 'release', DEFAULT_RUST_BIN_NAME),
  ];

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }

  const installed = getInstalledBinaryPath();
  if (fs.existsSync(installed)) {
    return installed;
  }

  return DEFAULT_RUST_BIN_NAME;
}

function buildRustTuiCommand(options = {}) {
  const initialScreen = resolveInitialScreen(options);
  const providerOverride = resolveTuiProviderOverride(options);
  const uiVariant = resolveUiVariant(options);
  const command = options.binaryPath || resolveRustTuiBinary();
  const args = [];

  if (initialScreen) {
    args.push('--initial-screen', initialScreen);
  }

  if (uiVariant) {
    args.push('--ui', uiVariant);
  }

  if (providerOverride) {
    args.push('--provider-override', providerOverride);
  }

  const env = { ...process.env };
  if (initialScreen) {
    env[TUI_INITIAL_SCREEN_ENV] = initialScreen;
  }
  if (uiVariant) {
    env[TUI_UI_VARIANT_ENV] = uiVariant;
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
        `Set ${TUI_BINARY_ENV} or ZEROSHOT_TUI_BINARY_PATH to a valid Rust TUI binary.`
      );
      process.exitCode = 1;
    });
  }

  return child;
}

function launchTuiSession(options = {}) {
  return launchRustTui(options);
}

module.exports = {
  buildRustTuiCommand,
  launchRustTui,
  launchTuiSession,
  resolveInitialScreen,
  resolveRustTuiBinary,
  resolveTuiProviderOverride,
  resolveUiVariant,
};
