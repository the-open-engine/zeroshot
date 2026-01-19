const PROVIDER_ALIASES = {
  anthropic: 'claude',
  openai: 'codex',
  google: 'gemini',
  claude: 'claude',
  codex: 'codex',
  gemini: 'gemini',
  opencode: 'opencode',
};

const VALID_PROVIDERS = ['claude', 'codex', 'gemini', 'opencode'];

function normalizeProviderName(name) {
  if (!name || typeof name !== 'string') return name;
  const normalized = PROVIDER_ALIASES[name.toLowerCase()];
  return normalized || name;
}

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
