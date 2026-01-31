const path = require('path');
const { execSync } = require('child_process');
const chalk = require('chalk');
const { normalizeProviderName } = require('./provider-names');
const { getProvider } = require('../src/providers');

const PACKAGE_ROOT = path.resolve(__dirname, '..');

/**
 * Detect git repository root from current directory.
 * @returns {string} Git repo root, or process.cwd() if not in a git repo.
 */
function detectGitRepoRoot() {
  try {
    const root = execSync('git rev-parse --show-toplevel', {
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'pipe'],
    }).trim();
    return root;
  } catch {
    return process.cwd();
  }
}

function resolveOptionalString(value) {
  if (typeof value !== 'string') return undefined;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function resolveEnvBool(value) {
  if (typeof value !== 'string') return undefined;
  const trimmed = value.trim().toLowerCase();
  if (trimmed === '1' || trimmed === 'true' || trimmed === 'yes') return true;
  if (trimmed === '0' || trimmed === 'false' || trimmed === 'no') return false;
  return undefined;
}

function resolveCloseIssueMode(value) {
  const trimmed = resolveOptionalString(value);
  if (!trimmed) return undefined;
  const normalized = trimmed.toLowerCase();
  if (normalized === 'auto' || normalized === 'always' || normalized === 'never') {
    return normalized;
  }
  return undefined;
}

function parseRunOptionsEnv() {
  const raw = resolveOptionalString(process.env.ZEROSHOT_RUN_OPTIONS);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object') {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

/**
 * Parse CLI mount specs (host:container[:ro]) into mount config objects.
 * @param {string[]} specs - Array of mount specs from CLI.
 * @returns {Array<{host: string, container: string, readonly: boolean}>}
 */
function parseMountSpecs(specs) {
  return specs.map((spec) => {
    const parts = spec.split(':');
    if (parts.length < 2) {
      throw new Error(`Invalid mount spec: "${spec}". Format: host:container[:ro]`);
    }
    const host = parts[0];
    const container = parts[1];
    const readonly = parts[2] === 'ro';
    return { host, container, readonly };
  });
}

function buildTextInput(text) {
  return { text };
}

function buildIssueInput(issue) {
  return { issue };
}

function buildFileInput(file) {
  return { file };
}

function detectRunInput(inputArg) {
  const isGitHubUrl = /^https?:\/\/github\.com\/[\w-]+\/[\w-]+\/issues\/\d+/.test(inputArg);
  const isGitLabUrl = /gitlab\.(com|[\w.-]+)\/[\w-]+\/[\w-]+\/-\/issues\/\d+/.test(inputArg);
  const isJiraUrl = /(atlassian\.net|jira\.[\w.-]+)\/browse\/[A-Z][A-Z0-9]+-\d+/.test(inputArg);
  const isAzureUrl =
    /dev\.azure\.com\/.*\/_workitems\/edit\/\d+/.test(inputArg) ||
    /visualstudio\.com\/.*\/_workitems\/edit\/\d+/.test(inputArg);
  const isJiraKey = /^[A-Z][A-Z0-9]+-\d+$/.test(inputArg);
  const isIssueNumber = /^\d+$/.test(inputArg);
  const isRepoIssue = /^[\w-]+\/[\w-]+#\d+$/.test(inputArg);
  const isMarkdownFile = /\.(md|markdown)$/i.test(inputArg);

  if (
    isGitHubUrl ||
    isGitLabUrl ||
    isJiraUrl ||
    isAzureUrl ||
    isJiraKey ||
    isIssueNumber ||
    isRepoIssue
  ) {
    return buildIssueInput(inputArg);
  }
  if (isMarkdownFile) {
    return buildFileInput(inputArg);
  }
  return buildTextInput(inputArg);
}

function resolveProviderOverride(options = {}) {
  const override = options.provider || options.envProvider || process.env.ZEROSHOT_PROVIDER;
  if (!override || (typeof override === 'string' && !override.trim())) {
    return null;
  }
  const normalized = normalizeProviderName(override);
  if (options.validateProvider) {
    getProvider(normalized);
  }
  return normalized;
}

function resolveConfigPath(configName) {
  if (path.isAbsolute(configName) || configName.startsWith('./') || configName.startsWith('../')) {
    return path.resolve(process.cwd(), configName);
  }
  if (configName.endsWith('.json')) {
    return path.join(PACKAGE_ROOT, 'cluster-templates', configName);
  }
  return path.join(PACKAGE_ROOT, 'cluster-templates', `${configName}.json`);
}

function ensureConfigProviderDefaults(config, settings) {
  if (!config.defaultProvider) {
    config.defaultProvider = settings.defaultProvider || 'claude';
  }
  config.defaultProvider = normalizeProviderName(config.defaultProvider) || 'claude';
}

function applyProviderOverrideToConfig(config, providerOverride, settings) {
  const provider = getProvider(providerOverride);
  const providerSettings = settings.providerSettings?.[providerOverride] || {};
  config.forceProvider = providerOverride;
  config.defaultProvider = providerOverride;
  config.forceLevel = providerSettings.defaultLevel || provider.getDefaultLevel();
  config.defaultLevel = config.forceLevel;
  console.log(chalk.dim(`Provider override: ${providerOverride} (all agents)`));
}

function loadClusterConfig(orchestrator, configPath, settings = {}, providerOverride) {
  const config = orchestrator.loadConfig(configPath);
  ensureConfigProviderDefaults(config, settings);
  if (providerOverride) {
    applyProviderOverrideToConfig(config, providerOverride, settings);
  }
  return config;
}

function buildStartOptions({
  clusterId,
  options = {},
  settings = {},
  providerOverride,
  modelOverride,
  forceProvider,
}) {
  const envRunOptions = parseRunOptionsEnv();
  const mergedOptions = envRunOptions ? { ...envRunOptions, ...options } : options;
  const targetCwd = process.env.ZEROSHOT_CWD || detectGitRepoRoot();
  const envPrBase = resolveOptionalString(process.env.ZEROSHOT_PR_BASE);
  const envMergeQueue = resolveEnvBool(process.env.ZEROSHOT_MERGE_QUEUE);
  const envCloseIssue = resolveCloseIssueMode(process.env.ZEROSHOT_CLOSE_ISSUE);
  const prBase = resolveOptionalString(mergedOptions.prBase) || envPrBase;
  const mergeQueue =
    typeof mergedOptions.mergeQueue === 'boolean' ? mergedOptions.mergeQueue : envMergeQueue;
  const closeIssue = resolveCloseIssueMode(mergedOptions.closeIssue) || envCloseIssue;
  return {
    clusterId,
    cwd: targetCwd,
    isolation:
      mergedOptions.docker || process.env.ZEROSHOT_DOCKER === '1' || settings.defaultDocker,
    isolationImage: mergedOptions.dockerImage || process.env.ZEROSHOT_DOCKER_IMAGE || undefined,
    worktree: mergedOptions.worktree || process.env.ZEROSHOT_WORKTREE === '1',
    autoPr: mergedOptions.pr || process.env.ZEROSHOT_PR === '1',
    autoMerge: process.env.ZEROSHOT_MERGE === '1',
    autoPush: process.env.ZEROSHOT_PUSH === '1',
    modelOverride: modelOverride || undefined,
    providerOverride: providerOverride || undefined,
    noMounts: mergedOptions.mounts === false || mergedOptions.noMounts === true,
    mounts: mergedOptions.mount ? parseMountSpecs(mergedOptions.mount) : undefined,
    containerHome: mergedOptions.containerHome || undefined,
    forceProvider: forceProvider || undefined,
    // PR configuration options (CLI args)
    prBase: prBase || undefined,
    mergeQueue: mergeQueue,
    closeIssue: closeIssue || undefined,
    settings,
  };
}

function resolveConfigOrThrow({
  orchestrator,
  config,
  configPath,
  configName,
  settings,
  providerOverride,
}) {
  if (config) {
    return config;
  }
  const resolvedPath = configPath || (configName ? resolveConfigPath(configName) : null);
  if (!resolvedPath) {
    throw new Error('configPath or configName is required when config is not provided');
  }
  return loadClusterConfig(orchestrator, resolvedPath, settings, providerOverride);
}

function startClusterFromText({
  orchestrator,
  text,
  config,
  configPath,
  configName,
  settings,
  providerOverride,
  modelOverride,
  forceProvider,
  clusterId,
  options = {},
}) {
  if (!orchestrator) {
    throw new Error('orchestrator is required');
  }
  const resolvedConfig = resolveConfigOrThrow({
    orchestrator,
    config,
    configPath,
    configName,
    settings,
    providerOverride,
  });
  const startOptions = buildStartOptions({
    clusterId,
    options,
    settings,
    providerOverride,
    modelOverride,
    forceProvider,
  });
  return orchestrator.start(resolvedConfig, buildTextInput(text), startOptions);
}

function startClusterFromIssue({
  orchestrator,
  issue,
  config,
  configPath,
  configName,
  settings,
  providerOverride,
  modelOverride,
  forceProvider,
  clusterId,
  options = {},
}) {
  if (!orchestrator) {
    throw new Error('orchestrator is required');
  }
  const resolvedConfig = resolveConfigOrThrow({
    orchestrator,
    config,
    configPath,
    configName,
    settings,
    providerOverride,
  });
  const startOptions = buildStartOptions({
    clusterId,
    options,
    settings,
    providerOverride,
    modelOverride,
    forceProvider,
  });
  return orchestrator.start(resolvedConfig, buildIssueInput(issue), startOptions);
}

function startClusterFromFile({
  orchestrator,
  file,
  config,
  configPath,
  configName,
  settings,
  providerOverride,
  modelOverride,
  forceProvider,
  clusterId,
  options = {},
}) {
  if (!orchestrator) {
    throw new Error('orchestrator is required');
  }
  const resolvedConfig = resolveConfigOrThrow({
    orchestrator,
    config,
    configPath,
    configName,
    settings,
    providerOverride,
  });
  const startOptions = buildStartOptions({
    clusterId,
    options,
    settings,
    providerOverride,
    modelOverride,
    forceProvider,
  });
  return orchestrator.start(resolvedConfig, buildFileInput(file), startOptions);
}

module.exports = {
  buildTextInput,
  buildIssueInput,
  buildFileInput,
  detectRunInput,
  resolveProviderOverride,
  resolveConfigPath,
  loadClusterConfig,
  buildStartOptions,
  startClusterFromText,
  startClusterFromIssue,
  startClusterFromFile,
  detectGitRepoRoot,
};
