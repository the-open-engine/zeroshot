const path = require('path');
const { spawnSync } = require('child_process');
const chalk = require('chalk');
const { normalizeProviderName } = require('./provider-names');
const { getProvider } = require('../src/providers');
const TemplateResolver = require('../src/template-resolver');

const PACKAGE_ROOT = path.resolve(__dirname, '..');

function firstTruthy(...values) {
  return values.find(Boolean);
}
function anyTruthy(...values) {
  return values.some(Boolean);
}
function optionalValue(value) {
  return value || undefined;
}
function resolveTargetCwd() {
  return firstTruthy(process.env.ZEROSHOT_CWD, detectGitRepoRoot());
}

function detectGitRepoRoot() {
  try {
    const result = spawnSync('git', ['rev-parse', '--show-toplevel'], {
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    if (result.status !== 0) {
      return process.cwd();
    }
    return result.stdout.trim();
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

const STDIN_MARKER = '-';

function isStdinInput(inputArg) {
  return inputArg === STDIN_MARKER;
}

async function readStdinText(stream = process.stdin) {
  const chunks = [];
  for await (const chunk of stream) {
    chunks.push(chunk);
  }
  return Buffer.concat(chunks.map((chunk) => (Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk))))
    .toString('utf8')
    .trim();
}

function encodeStdinEnv(text) {
  return Buffer.from(text, 'utf8').toString('base64');
}

function decodeStdinEnv(value) {
  return Buffer.from(value, 'base64').toString('utf8');
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

function resolveParameterizedConfigFile(config) {
  if (!config?.params || Object.keys(config.params).length === 0) {
    return config;
  }

  const resolver = new TemplateResolver(path.join(PACKAGE_ROOT, 'cluster-templates'));
  return resolver.resolveTemplate(config, {});
}

function loadClusterConfig(orchestrator, configPath, settings = {}, providerOverride) {
  const config = resolveParameterizedConfigFile(orchestrator.loadConfig(configPath));
  ensureConfigProviderDefaults(config, settings);
  if (providerOverride) {
    applyProviderOverrideToConfig(config, providerOverride, settings);
  }
  return config;
}

function mergeRunOptions(options) {
  const envRunOptions = parseRunOptionsEnv();
  return envRunOptions ? { ...envRunOptions, ...options } : options;
}

function resolveIsolation(mergedOptions, settings) {
  return mergedOptions.docker || process.env.ZEROSHOT_DOCKER === '1' || settings.defaultDocker;
}

function resolveMergeQueue(mergedOptions) {
  if (typeof mergedOptions.mergeQueue === 'boolean') {
    return mergedOptions.mergeQueue;
  }
  return resolveEnvBool(process.env.ZEROSHOT_MERGE_QUEUE);
}

function resolvePrBase(mergedOptions) {
  return (
    resolveOptionalString(mergedOptions.prBase) ||
    resolveOptionalString(process.env.ZEROSHOT_PR_BASE) ||
    undefined
  );
}

function resolveCloseIssue(mergedOptions) {
  return (
    resolveCloseIssueMode(mergedOptions.closeIssue) ||
    resolveCloseIssueMode(process.env.ZEROSHOT_CLOSE_ISSUE) ||
    undefined
  );
}

function resolveMounts(mergedOptions) {
  return mergedOptions.mount ? parseMountSpecs(mergedOptions.mount) : undefined;
}

function buildStartOptions({
  clusterId,
  options = {},
  settings = {},
  providerOverride,
  modelOverride,
  forceProvider,
}) {
  const mergedOptions = mergeRunOptions(options);
  return {
    clusterId,
    cwd: resolveTargetCwd(),
    isolation: resolveIsolation(mergedOptions, settings),
    isolationImage: firstTruthy(mergedOptions.dockerImage, process.env.ZEROSHOT_DOCKER_IMAGE),
    worktree: anyTruthy(mergedOptions.worktree, process.env.ZEROSHOT_WORKTREE === '1'),
    autoPr: anyTruthy(mergedOptions.pr, process.env.ZEROSHOT_PR === '1'),
    autoMerge: process.env.ZEROSHOT_MERGE === '1',
    autoPush: process.env.ZEROSHOT_PUSH === '1',
    modelOverride: optionalValue(modelOverride),
    providerOverride: optionalValue(providerOverride),
    noMounts: anyTruthy(mergedOptions.mounts === false, mergedOptions.noMounts === true),
    mounts: resolveMounts(mergedOptions),
    containerHome: optionalValue(mergedOptions.containerHome),
    forceProvider: optionalValue(forceProvider),
    prBase: resolvePrBase(mergedOptions),
    mergeQueue: resolveMergeQueue(mergedOptions),
    closeIssue: resolveCloseIssue(mergedOptions),
    ship: mergedOptions.ship,
    requiredQualityGates: mergedOptions.requiredQualityGates,
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

function startClusterWithInput(args, input) {
  const { orchestrator, config, configPath, configName, settings, providerOverride } = args;
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
    clusterId: args.clusterId,
    options: args.options || {},
    settings,
    providerOverride,
    modelOverride: args.modelOverride,
    forceProvider: args.forceProvider,
  });
  return orchestrator.start(resolvedConfig, input, startOptions);
}

function startClusterFromText(args) {
  return startClusterWithInput(args, buildTextInput(args.text));
}

function startClusterFromIssue(args) {
  return startClusterWithInput(args, buildIssueInput(args.issue));
}

function startClusterFromFile(args) {
  return startClusterWithInput(args, buildFileInput(args.file));
}

module.exports = {
  buildTextInput,
  buildIssueInput,
  buildFileInput,
  detectRunInput,
  isStdinInput,
  readStdinText,
  encodeStdinEnv,
  decodeStdinEnv,
  resolveProviderOverride,
  resolveConfigPath,
  loadClusterConfig,
  buildStartOptions,
  startClusterFromText,
  startClusterFromIssue,
  startClusterFromFile,
  detectGitRepoRoot,
};
