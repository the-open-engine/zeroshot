/**
 * Copilot output parser.
 *
 * GitHub Copilot CLI (`copilot -p ... --silent`) emits plain text on stdout
 * (not structured JSON). Our parser:
 *   - Tries JSON.parse on each line in case a future format ever ships
 *     structured events (best-effort; ignored on failure).
 *   - Otherwise treats any non-empty line as a `{ type: 'text', text }` event.
 *   - Does NOT emit a synthetic `result` event; the agent wrapper handles
 *     completion via process exit.
 */

function tryParseStructured(trimmed) {
  if (!(trimmed.startsWith('{') || trimmed.startsWith('['))) return null;
  let event;
  try {
    event = JSON.parse(trimmed);
  } catch {
    return null;
  }
  if (!event || typeof event !== 'object') return null;

  if (event.type === 'text' && typeof event.text === 'string') {
    return { type: 'text', text: event.text };
  }
  if (event.type === 'result') {
    return {
      type: 'result',
      success: event.success !== false,
      inputTokens: event.inputTokens || 0,
      outputTokens: event.outputTokens || 0,
      error: event.error,
    };
  }
  if (event.type === 'error') {
    return {
      type: 'result',
      success: false,
      error: event.error || event.message || 'Unknown error',
    };
  }
  return null;
}

function parseEvent(line) {
  if (line === null || line === undefined) return null;
  const trimmed = String(line).trim();
  if (!trimmed) return null;

  const structured = tryParseStructured(trimmed);
  if (structured) return structured;

  return { type: 'text', text: trimmed };
}

function parseChunk(chunk) {
  if (!chunk) return [];
  const events = [];
  const lines = String(chunk).split('\n');
  for (const line of lines) {
    const event = parseEvent(line);
    if (!event) continue;
    if (Array.isArray(event)) {
      events.push(...event);
    } else {
      events.push(event);
    }
  }
  return events;
}

module.exports = {
  parseEvent,
  parseChunk,
};
