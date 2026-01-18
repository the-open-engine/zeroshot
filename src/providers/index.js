const AnthropicProvider = require('./anthropic');
const OpenAIProvider = require('./openai');
const GoogleProvider = require('./google');
const OpencodeProvider = require('./opencode');
const { normalizeProviderName } = require('../../lib/provider-names');

const PROVIDERS = {
  claude: AnthropicProvider,
  codex: OpenAIProvider,
  gemini: GoogleProvider,
  opencode: OpencodeProvider,
};

function getProvider(name) {
  const normalized = normalizeProviderName(name || '');
  const Provider = PROVIDERS[normalized];
  if (!Provider) {
    throw new Error(`Unknown provider: ${name}. Valid: ${Object.keys(PROVIDERS).join(', ')}`);
  }
  return new Provider();
}

async function detectProviders() {
  const results = {};
  for (const [name, Provider] of Object.entries(PROVIDERS)) {
    const provider = new Provider();
    results[name] = {
      available: await provider.isAvailable(),
    };
  }
  return results;
}

function listProviders() {
  return Object.keys(PROVIDERS);
}

function stripTimestampPrefix(line) {
  if (!line || typeof line !== 'string') return '';
  let trimmed = line.trim().replace(/\r$/, '');
  if (!trimmed) return '';

  const tsMatch = trimmed.match(/^\[(\d{13})\](.*)$/);
  if (tsMatch) trimmed = (tsMatch[2] || '').trimStart();

  // In cluster logs, lines are often prefixed like:
  // "validator       | {json...}"
  // Strip the "<agent> | " prefix so provider parsers can JSON.parse the event.
  if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) {
    const pipeMatch = trimmed.match(/^[^|]{1,40}\|\s*(.*)$/);
    if (pipeMatch) {
      const afterPipe = (pipeMatch[1] || '').trimStart();
      if (afterPipe.startsWith('{') || afterPipe.startsWith('[')) return afterPipe;
    }
  }

  return trimmed;
}

function parseChunkWithProvider(provider, chunk) {
  if (!chunk) return [];
  const events = [];
  const lines = chunk.split('\n');

  for (const line of lines) {
    const content = stripTimestampPrefix(line);
    if (!content) continue;
    const event = provider.parseEvent(content);
    if (!event) continue;
    if (Array.isArray(event)) {
      events.push(...event);
    } else {
      events.push(event);
    }
  }

  return events;
}

function parseProviderChunk(providerName, chunk) {
  const provider = getProvider(providerName || 'claude');
  return parseChunkWithProvider(provider, chunk);
}

module.exports = {
  getProvider,
  detectProviders,
  listProviders,
  parseProviderChunk,
  parseChunkWithProvider,
};
