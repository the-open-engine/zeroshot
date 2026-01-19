const { normalizeProviderName } = require('../../lib/provider-names');

const CAPABILITIES = {
  claude: {
    dockerIsolation: true,
    worktreeIsolation: true,
    mcpServers: true,
    jsonSchema: true,
    streamJson: true,
    thinkingMode: true,
    reasoningEffort: false,
  },
  codex: {
    dockerIsolation: true,
    worktreeIsolation: true,
    mcpServers: true,
    jsonSchema: true,
    streamJson: true,
    thinkingMode: true,
    reasoningEffort: true,
  },
  gemini: {
    dockerIsolation: true,
    worktreeIsolation: true,
    mcpServers: true,
    jsonSchema: 'experimental',
    streamJson: true,
    thinkingMode: true,
    reasoningEffort: false,
  },
  opencode: {
    dockerIsolation: true,
    worktreeIsolation: true,
    mcpServers: true,
    jsonSchema: 'experimental',
    streamJson: true,
    thinkingMode: true,
    reasoningEffort: true,
  },
};

function checkCapability(provider, capability) {
  const caps = CAPABILITIES[normalizeProviderName(provider)];
  if (!caps) return false;
  return caps[capability] === true;
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
