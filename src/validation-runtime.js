const { readRepoSettings } = require('../lib/repo-settings');

const VALIDATION_RUNTIME_STATUS = Object.freeze({
  DISABLED: 'disabled',
  NOT_STARTED: 'not_started',
  STARTING: 'starting',
  READY: 'ready',
  FAILED: 'failed',
  STOPPING: 'stopping',
});

const DEFAULT_BOOT_TIMEOUT_MS = 5 * 60 * 1000;
const DEFAULT_READY_TIMEOUT_MS = 2 * 60 * 1000;
const DEFAULT_READY_INTERVAL_MS = 2000;
const DEFAULT_TEARDOWN_TIMEOUT_MS = 60 * 1000;

function normalizePositiveInt(value, fallback) {
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function normalizeValidationRuntimeConfig(runtimeConfig) {
  if (!runtimeConfig || typeof runtimeConfig !== 'object' || Array.isArray(runtimeConfig)) {
    throw new Error('validationRuntime must be an object');
  }

  if (typeof runtimeConfig.boot !== 'string' || !runtimeConfig.boot.trim()) {
    throw new Error('validationRuntime.boot must be a non-empty string');
  }

  if (!Array.isArray(runtimeConfig.ready) || runtimeConfig.ready.length === 0) {
    throw new Error('validationRuntime.ready must be a non-empty array');
  }

  const ready = runtimeConfig.ready.map((command, index) => {
    if (typeof command !== 'string' || !command.trim()) {
      throw new Error(`validationRuntime.ready[${index}] must be a non-empty string`);
    }
    return command.trim();
  });

  if (typeof runtimeConfig.teardown !== 'string' || !runtimeConfig.teardown.trim()) {
    throw new Error('validationRuntime.teardown must be a non-empty string');
  }

  const env = runtimeConfig.env;
  if (env !== undefined && (typeof env !== 'object' || env === null || Array.isArray(env))) {
    throw new Error('validationRuntime.env must be an object when provided');
  }

  return {
    env: env || {},
    boot: runtimeConfig.boot.trim(),
    ready,
    teardown: runtimeConfig.teardown.trim(),
    bootTimeoutMs: normalizePositiveInt(runtimeConfig.bootTimeoutMs, DEFAULT_BOOT_TIMEOUT_MS),
    readyTimeoutMs: normalizePositiveInt(runtimeConfig.readyTimeoutMs, DEFAULT_READY_TIMEOUT_MS),
    readyIntervalMs: normalizePositiveInt(
      runtimeConfig.readyIntervalMs,
      DEFAULT_READY_INTERVAL_MS
    ),
    teardownTimeoutMs: normalizePositiveInt(
      runtimeConfig.teardownTimeoutMs,
      DEFAULT_TEARDOWN_TIMEOUT_MS
    ),
  };
}

function readValidationRuntimeSettings(startDir) {
  const repoSettings = readRepoSettings(startDir);
  const runtimeConfig = repoSettings.settings?.validationRuntime;

  if (!runtimeConfig) {
    return {
      enabled: false,
      repoRoot: repoSettings.repoRoot,
      settingsPath: repoSettings.settingsPath,
      config: null,
    };
  }

  return {
    enabled: true,
    repoRoot: repoSettings.repoRoot,
    settingsPath: repoSettings.settingsPath,
    config: normalizeValidationRuntimeConfig(runtimeConfig),
  };
}

function getValidationRuntimePortKeys(runtimeConfig) {
  const env = runtimeConfig?.env || {};
  const keys = new Set();

  for (const value of Object.values(env)) {
    if (typeof value !== 'string') {
      continue;
    }

    for (const match of value.matchAll(/\{\{\s*ports\.([A-Za-z0-9_]+)\s*\}\}/g)) {
      keys.add(match[1]);
    }
  }

  return Array.from(keys).sort();
}

function resolveValidationRuntimeEnv({ envConfig, clusterId, allocatedPorts = {} }) {
  const resolved = {};

  for (const [key, value] of Object.entries(envConfig || {})) {
    if (value === null || value === undefined) {
      continue;
    }

    if (typeof value !== 'string') {
      resolved[key] = String(value);
      continue;
    }

    resolved[key] = value
      .replace(/\{\{\s*clusterId\s*\}\}/g, clusterId)
      .replace(/\{\{\s*ports\.([A-Za-z0-9_]+)\s*\}\}/g, (_match, portKey) => {
        if (!Object.prototype.hasOwnProperty.call(allocatedPorts, portKey)) {
          throw new Error(`validationRuntime.env requested missing port bundle key '${portKey}'`);
        }
        return String(allocatedPorts[portKey]);
      });
  }

  return resolved;
}

function getValidationRuntimeTemplateParams(enabled) {
  if (enabled) {
    return {
      include_runtime_validator: true,
      heavy_validator_count: 3,
      heavy_validator_ids_js: '["validator-security","validator-tester","validator-runtime"]',
    };
  }

  return {
    include_runtime_validator: false,
    heavy_validator_count: 2,
    heavy_validator_ids_js: '["validator-security","validator-tester"]',
  };
}

module.exports = {
  VALIDATION_RUNTIME_STATUS,
  DEFAULT_BOOT_TIMEOUT_MS,
  DEFAULT_READY_TIMEOUT_MS,
  DEFAULT_READY_INTERVAL_MS,
  DEFAULT_TEARDOWN_TIMEOUT_MS,
  getValidationRuntimePortKeys,
  getValidationRuntimeTemplateParams,
  normalizeValidationRuntimeConfig,
  readValidationRuntimeSettings,
  resolveValidationRuntimeEnv,
};
