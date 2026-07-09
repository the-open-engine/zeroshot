const {
  PROVIDER_CAPABILITIES,
  normalizeProviderName,
  providerSupportsCapability,
} = require('../../lib/provider-names');

const CAPABILITIES = Object.freeze(
  Object.fromEntries(
    Object.entries(PROVIDER_CAPABILITIES).map(([provider, capabilities]) => [
      provider,
      Object.freeze({ ...capabilities }),
    ])
  )
);

function checkCapability(provider, capability) {
  if (!provider) return false;
  return providerSupportsCapability(provider, capability);
}

function warnIfExperimental(provider, capability) {
  const normalized = normalizeProviderName(provider);
  const caps = CAPABILITIES[normalized];
  if (caps?.[capability] === 'experimental') {
    console.warn(`⚠️ ${capability} is experimental for ${normalized} and may not work reliably`);
  }
}

module.exports = {
  CAPABILITIES,
  checkCapability,
  warnIfExperimental,
};
