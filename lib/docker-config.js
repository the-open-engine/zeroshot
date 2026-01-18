/**
 * Docker mount configuration for isolation mode
 * Fully generic - no hardcoded paths, all configurable
 */

/**
 * Built-in mount presets
 * Uses $HOME placeholder - resolved at runtime based on dockerContainerHome setting
 * Host paths use ~ (expanded to host user's home)
 * Container paths use $HOME (expanded to configured container home)
 */
const MOUNT_PRESETS = {
  gh: { host: '~/.config/gh', container: '$HOME/.config/gh', readonly: false },
  git: { host: '~/.gitconfig', container: '$HOME/.gitconfig', readonly: true },
  ssh: { host: '~/.ssh', container: '$HOME/.ssh', readonly: true },
  aws: { host: '~/.aws', container: '$HOME/.aws', readonly: true },
  azure: { host: '~/.azure', container: '$HOME/.azure', readonly: true },
  kube: { host: '~/.kube', container: '$HOME/.kube', readonly: true },
  terraform: { host: '~/.terraform.d', container: '$HOME/.terraform.d', readonly: false },
  gcloud: { host: '~/.config/gcloud', container: '$HOME/.config/gcloud', readonly: true },
  claude: { host: '~/.claude', container: '$HOME/.claude', readonly: true },
  codex: { host: '~/.config/codex', container: '$HOME/.config/codex', readonly: true },
  gemini: { host: '~/.config/gemini', container: '$HOME/.config/gemini', readonly: true },
  opencode: {
    host: '~/.local/share/opencode',
    container: '$HOME/.local/share/opencode',
    readonly: true,
  },
};

/**
 * Environment variables to auto-pass for each preset
 * Supports:
 * - Simple: 'VAR_NAME' (pass if set in host env)
 * - Pattern: 'VAR_*' (pass all matching vars)
 * - Forced: 'VAR=value' (always set to value)
 * - Empty: 'VAR=' (always set to empty string)
 */
const { CLAUDE_AUTH_ENV_VARS } = require('./settings/claude-auth');

const ENV_PRESETS = {
  aws: ['AWS_REGION', 'AWS_DEFAULT_REGION', 'AWS_PROFILE', 'AWS_PAGER='],
  azure: ['AZURE_SUBSCRIPTION_ID', 'AZURE_TENANT_ID', 'AZURE_CLIENT_ID'],
  gcloud: ['CLOUDSDK_CORE_PROJECT', 'GOOGLE_CLOUD_PROJECT'],
  kube: ['KUBECONFIG'],
  terraform: ['TF_VAR_*'],
  claude: CLAUDE_AUTH_ENV_VARS,
};

/**
 * Resolve mount config to actual mount specs
 * @param {Array<string|object>} config - Preset names or {host, container, readonly?} objects
 * @param {object} options - Resolution options
 * @param {string} [options.containerHome='/root'] - Container home directory for $HOME expansion
 * @returns {Array<{host: string, container: string, readonly: boolean}>}
 */
function resolveMounts(config, options = {}) {
  if (!Array.isArray(config)) {
    throw new Error('dockerMounts must be an array');
  }

  const containerHome = options.containerHome || '/root';

  return config.flatMap((item) => {
    if (typeof item === 'string') {
      const preset = MOUNT_PRESETS[item];
      if (!preset) {
        throw new Error(
          `Unknown mount preset: "${item}". Valid presets: ${Object.keys(MOUNT_PRESETS).join(', ')}`
        );
      }
      return {
        host: preset.host,
        container: preset.container.replace(/\$HOME/g, containerHome),
        readonly: preset.readonly,
      };
    }

    if (typeof item === 'object' && item !== null) {
      if (!item.host || !item.container) {
        throw new Error('Custom mount must have "host" and "container" properties');
      }
      return {
        host: item.host,
        container: item.container.replace(/\$HOME/g, containerHome),
        readonly: item.readonly !== false,
      };
    }

    throw new Error(
      `Invalid mount config: ${JSON.stringify(item)}. Use preset name or {host, container, readonly?}`
    );
  });
}

/**
 * Resolve env vars to pass based on enabled presets + explicit additions
 * @param {Array<string|object>} mountConfig - The mount config (to detect presets)
 * @param {Array<string>} extraEnvs - Additional env vars to pass
 * @returns {Array<string>} - List of env var specs
 */
function resolveEnvs(mountConfig, extraEnvs = []) {
  const envs = new Set(extraEnvs);

  for (const item of mountConfig) {
    if (typeof item === 'string' && ENV_PRESETS[item]) {
      for (const envVar of ENV_PRESETS[item]) {
        envs.add(envVar);
      }
    }
  }

  return [...envs];
}

/**
 * Expand env var patterns and resolve values
 * Supports:
 * - Simple: 'VAR_NAME' (use value from env if set)
 * - Pattern: 'VAR_*' (expand to all matching vars)
 * - Forced: 'VAR=value' (always set to value)
 * - Empty: 'VAR=' (always set to empty string)
 *
 * @param {Array<string>} envVars - List of env var specs
 * @param {object} env - Environment object (defaults to process.env)
 * @returns {Array<{name: string, value: string|null, forced: boolean}>}
 */
function expandEnvPatterns(envVars, env = process.env) {
  const result = [];

  for (const envVar of envVars) {
    // Forced value: VAR=value or VAR=
    if (envVar.includes('=')) {
      const [name, ...valueParts] = envVar.split('=');
      const value = valueParts.join('=');
      result.push({ name, value, forced: true });
    }
    // Pattern: VAR_*
    else if (envVar.endsWith('*')) {
      const prefix = envVar.slice(0, -1);
      for (const key of Object.keys(env)) {
        if (key.startsWith(prefix)) {
          result.push({ name: key, value: null, forced: false });
        }
      }
    }
    // Simple: VAR_NAME
    else {
      result.push({ name: envVar, value: null, forced: false });
    }
  }

  return result;
}

/**
 * Validate mount config
 * @param {unknown} value - Value to validate
 * @returns {string|null} - Error message if invalid, null if valid
 */
function validateMountConfig(value) {
  if (!Array.isArray(value)) {
    return 'dockerMounts must be an array';
  }

  for (const item of value) {
    if (typeof item === 'string') {
      if (!MOUNT_PRESETS[item]) {
        return `Unknown mount preset: "${item}". Valid: ${Object.keys(MOUNT_PRESETS).join(', ')}`;
      }
    } else if (typeof item === 'object' && item !== null) {
      if (!item.host) {
        return 'Custom mount missing "host" property';
      }
      if (!item.container) {
        return 'Custom mount missing "container" property';
      }
      if (item.readonly !== undefined && typeof item.readonly !== 'boolean') {
        return '"readonly" must be a boolean';
      }
    } else {
      return `Invalid mount: ${JSON.stringify(item)}. Use preset name or {host, container, readonly?}`;
    }
  }

  return null;
}

/**
 * Validate env passthrough config
 * @param {unknown} value - Value to validate
 * @returns {string|null} - Error message if invalid, null if valid
 */
function validateEnvPassthrough(value) {
  if (!Array.isArray(value)) {
    return 'dockerEnvPassthrough must be an array';
  }

  for (const item of value) {
    if (typeof item !== 'string') {
      return `Invalid env var: ${JSON.stringify(item)}. Must be a string`;
    }
    // Allow: VAR, VAR_*, VAR=value, VAR=
    if (!/^[A-Z_][A-Z0-9_]*(\*|=.*)?$/i.test(item)) {
      return `Invalid env var spec: "${item}". Use VAR, VAR_*, or VAR=value`;
    }
  }

  return null;
}

module.exports = {
  MOUNT_PRESETS,
  ENV_PRESETS,
  resolveMounts,
  resolveEnvs,
  expandEnvPatterns,
  validateMountConfig,
  validateEnvPassthrough,
};
