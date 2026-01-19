/**
 * Compatibility wrapper for Claude stream-json parsing.
 * Prefer provider-specific parsers in src/providers.
 */
const { parseEvent, parseChunk } = require('../src/providers/anthropic/output-parser');

module.exports = {
  parseEvent,
  parseChunk,
};
