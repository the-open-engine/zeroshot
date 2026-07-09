/**
 * Provider-agnostic stream-json parser for `zeroshot logs`.
 *
 * Provider parsing is delegated to the helper-backed runtime provider facade.
 */

const { getProvider, listProviders } = require('../src/providers');

function createProviderParsers() {
  return listProviders().map((name) => getProvider(name));
}

function stripTimestampPrefix(line) {
  if (!line || typeof line !== 'string') return '';
  let trimmed = line.trim().replace(/\r$/, '');
  if (!trimmed) return '';

  const tsMatch = trimmed.match(/^\[(\d{13})\](.*)$/);
  if (tsMatch) trimmed = (tsMatch[2] || '').trimStart();

  if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) {
    const pipeMatch = trimmed.match(/^[^|]{1,40}\|\s*(.*)$/);
    if (pipeMatch) {
      const afterPipe = (pipeMatch[1] || '').trimStart();
      if (afterPipe.startsWith('{') || afterPipe.startsWith('[')) return afterPipe;
    }
  }

  return trimmed;
}

function parseEvent(line, providerParsers = createProviderParsers()) {
  const content = stripTimestampPrefix(line);
  if (!content) return null;

  for (const provider of providerParsers) {
    const event = provider.parseEvent(content);
    if (event) return event;
  }

  return null;
}

function collectEvent(events, event) {
  if (!event) return;
  if (Array.isArray(event)) {
    events.push(...event);
    return;
  }
  events.push(event);
}

function parseChunk(chunk) {
  const events = [];
  const lines = String(chunk || '').split('\n');
  const providerParsers = createProviderParsers();

  for (const line of lines) {
    if (!line.trim()) continue;
    collectEvent(events, parseEvent(line, providerParsers));
  }

  return events;
}

module.exports = {
  parseEvent,
  parseChunk,
};
