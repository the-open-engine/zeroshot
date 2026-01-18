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
    // Prompt-level capabilities for template injection
    subAgents: true, // Can spawn Task tool with background agents
    parallelToolCalls: true, // Can call multiple tools in one response
    streaming: true,
    structuredOutput: true,
  },
  codex: {
    dockerIsolation: true,
    worktreeIsolation: true,
    mcpServers: true,
    jsonSchema: true,
    streamJson: true,
    thinkingMode: true,
    reasoningEffort: true,
    // Prompt-level capabilities for template injection
    subAgents: false,
    parallelToolCalls: false,
    streaming: false,
    structuredOutput: true,
  },
  gemini: {
    dockerIsolation: true,
    worktreeIsolation: true,
    mcpServers: true,
    jsonSchema: 'experimental',
    streamJson: true,
    thinkingMode: true,
    reasoningEffort: false,
    // Prompt-level capabilities for template injection
    subAgents: false,
    parallelToolCalls: true,
    streaming: true,
    structuredOutput: true,
  },
};

// Minimal fallback capabilities for unknown providers
const DEFAULT_CAPABILITIES = {
  dockerIsolation: false,
  worktreeIsolation: false,
  mcpServers: false,
  jsonSchema: false,
  streamJson: false,
  thinkingMode: false,
  reasoningEffort: false,
  subAgents: false,
  parallelToolCalls: false,
  streaming: false,
  structuredOutput: false,
};

function checkCapability(provider, capability) {
  const normalized = normalizeProviderName(provider);
  const caps = CAPABILITIES[normalized] || DEFAULT_CAPABILITIES;
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

/**
 * Get all capabilities for a provider (with fallback to defaults)
 * @param {string} provider - Provider name
 * @returns {Object} Capabilities object
 */
function getCapabilities(provider) {
  const normalized = normalizeProviderName(provider);
  return CAPABILITIES[normalized] || DEFAULT_CAPABILITIES;
}

module.exports = {
  CAPABILITIES,
  DEFAULT_CAPABILITIES,
  checkCapability,
  warnIfExperimental,
  getCapabilities,
};
