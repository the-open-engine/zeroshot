const { execSync } = require('child_process');

function detectGitRepoRoot() {
  try {
    return execSync('git rev-parse --show-toplevel', {
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'pipe'],
    }).trim();
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
    if (!parsed || typeof parsed !== 'object') return null;
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
    return { host: parts[0], container: parts[1], readonly: parts[2] === 'ro' };
  });
}

function mergeRunOptions(options = {}) {
  const envRunOptions = parseRunOptionsEnv();
  return envRunOptions ? { ...envRunOptions, ...options } : options;
}

function resolvePrOptions(mergedOptions) {
  const envPrBase = resolveOptionalString(process.env.ZEROSHOT_PR_BASE);
  const envMergeQueue = resolveEnvBool(process.env.ZEROSHOT_MERGE_QUEUE);
  const envCloseIssue = resolveCloseIssueMode(process.env.ZEROSHOT_CLOSE_ISSUE);

  const prBase = resolveOptionalString(mergedOptions.prBase) || envPrBase;
  const mergeQueue =
    typeof mergedOptions.mergeQueue === 'boolean' ? mergedOptions.mergeQueue : envMergeQueue;
  const closeIssue = resolveCloseIssueMode(mergedOptions.closeIssue) || envCloseIssue;

  return { prBase, mergeQueue, closeIssue };
}

function isEnvEnabled(envName) {
  return process.env[envName] === '1';
}

function isOptionOrEnvEnabled(optionValue, envName) {
  return Boolean(optionValue || isEnvEnabled(envName));
}

function resolveIsolation(mergedOptions, settings) {
  return Boolean(mergedOptions.docker || isEnvEnabled('ZEROSHOT_DOCKER') || settings.defaultDocker);
}

function resolveNoMounts(mergedOptions) {
  return mergedOptions.mounts === false || mergedOptions.noMounts === true;
}

function resolveRuntimeFlags(mergedOptions, settings) {
  return {
    cwd: process.env.ZEROSHOT_CWD || detectGitRepoRoot(),
    isolation: resolveIsolation(mergedOptions, settings),
    isolationImage: mergedOptions.dockerImage || process.env.ZEROSHOT_DOCKER_IMAGE || undefined,
    worktree: isOptionOrEnvEnabled(mergedOptions.worktree, 'ZEROSHOT_WORKTREE'),
    autoPr: isOptionOrEnvEnabled(mergedOptions.pr, 'ZEROSHOT_PR'),
    autoMerge: isEnvEnabled('ZEROSHOT_MERGE'),
    autoPush: isEnvEnabled('ZEROSHOT_PUSH'),
    noMounts: resolveNoMounts(mergedOptions),
    mounts: mergedOptions.mount ? parseMountSpecs(mergedOptions.mount) : undefined,
    containerHome: mergedOptions.containerHome || undefined,
  };
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
  const runtime = resolveRuntimeFlags(mergedOptions, settings);
  const pr = resolvePrOptions(mergedOptions);

  return {
    clusterId,
    ...runtime,
    modelOverride: modelOverride || undefined,
    providerOverride: providerOverride || undefined,
    forceProvider: forceProvider || undefined,
    prBase: pr.prBase || undefined,
    mergeQueue: pr.mergeQueue,
    closeIssue: pr.closeIssue || undefined,
    settings,
  };
}

module.exports = {
  buildStartOptions,
  detectGitRepoRoot,
};
