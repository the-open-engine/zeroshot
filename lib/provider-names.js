/**
 * Provider name aliases for normalization
 * Maps various provider name formats to canonical names
 * @type {Object<string, string>}
 */
const PROVIDER_ALIASES = {
  anthropic: 'claude',
  openai: 'codex',
  google: 'gemini',
  claude: 'claude',
  codex: 'codex',
  gemini: 'gemini',
  opencode: 'opencode',
};

/**
 * List of valid canonical provider names
 * @type {string[]}
 */
const VALID_PROVIDERS = ['claude', 'codex', 'gemini', 'opencode'];

/**
 * Normalize a provider name to its canonical form
 * @param {string} name - Provider name to normalize (e.g., 'anthropic', 'openai')
 * @returns {string} Canonical provider name or original input if not recognized
 * @example
 * normalizeProviderName('anthropic') // 'claude'
 * normalizeProviderName('openai')    // 'codex'
 * normalizeProviderName('Claude')    // 'claude' (case-insensitive)
 */
function normalizeProviderName(name) {
  if (!name || typeof name !== 'string') return name;
  const normalized = PROVIDER_ALIASES[name.toLowerCase()];
  return normalized || name;
}

/**
 * Normalize provider settings object keys to canonical provider names
 * Merges settings from aliases into their canonical provider keys
 * @param {Object} providerSettings - Provider settings object with provider names as keys
 * @returns {Object} Normalized provider settings with canonical names
 * @example
 * normalizeProviderSettings({ anthropic: { maxTokens: 100 }, openai: { model: 'gpt-4' } })
 * // { claude: { maxTokens: 100 }, codex: { model: 'gpt-4' } }
 */
function normalizeProviderSettings(providerSettings) {
  if (
    !providerSettings ||
    typeof providerSettings !== 'object' ||
    Array.isArray(providerSettings)
  ) {
    return providerSettings;
  }

  const normalized = {};
  const entries = Object.entries(providerSettings);
  const aliasFirst = entries.sort(([left], [right]) => {
    const leftIsCanonical = normalizeProviderName(left) === left;
    const rightIsCanonical = normalizeProviderName(right) === right;
    if (leftIsCanonical === rightIsCanonical) return 0;
    return leftIsCanonical ? 1 : -1;
  });

  for (const [key, value] of aliasFirst) {
    const canonical = normalizeProviderName(key);
    if (!VALID_PROVIDERS.includes(canonical)) {
      normalized[key] = value;
      continue;
    }
    normalized[canonical] = {
      ...(normalized[canonical] || {}),
      ...(value || {}),
    };
  }

  return normalized;
}

module.exports = {
  PROVIDER_ALIASES,
  VALID_PROVIDERS,
  normalizeProviderName,
  normalizeProviderSettings,
};
