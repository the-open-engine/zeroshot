/**
 * Provider-agnostic stream-json parser for `zeroshot logs`.
 *
 * Goals:
 * - Accept a single stream of JSONL events from multiple provider CLIs (Claude/Codex/Gemini/Opencode)
 * - Produce the common event types expected by the logs renderer:
 *   text, thinking, tool_call, tool_result, result, ...
 *
 * NOTE: Provider-specific parsers live in `src/providers/<provider>/output-parser.js`.
 * This file is a thin compatibility wrapper so the CLI can parse logs without
 * knowing the provider up front (task logs often don’t have provider metadata).
 */

const anthropic = require('../src/providers/anthropic/output-parser');
const openai = require('../src/providers/openai/output-parser');
const google = require('../src/providers/google/output-parser');
const opencode = require('../src/providers/opencode/output-parser');

const googleState = {};

function stripTimestampPrefix(line) {
  if (!line || typeof line !== 'string') return '';
  let trimmed = line.trim().replace(/\r$/, '');
  if (!trimmed) return '';

  const tsMatch = trimmed.match(/^\[(\d{13})\](.*)$/);
  if (tsMatch) trimmed = (tsMatch[2] || '').trimStart();

  // In cluster logs, lines can be prefixed like:
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

function parseEvent(line, options = {}) {
  const content = stripTimestampPrefix(line);
  if (!content) return null;

  // Try provider parsers in a stable order.
  // Each parser returns null for unknown event types, so the first non-null wins.
  return (
    anthropic.parseEvent(content, options) ||
    openai.parseEvent(content, options) ||
    opencode.parseEvent(content, options) ||
    google.parseEvent(content, googleState, options) ||
    null
  );
}

function parseChunk(chunk, options = {}) {
  const events = [];
  const lines = String(chunk || '').split('\n');

  for (const line of lines) {
    if (!line.trim()) continue;
    const event = parseEvent(line, options);
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
