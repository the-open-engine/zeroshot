/**
 * Output Extraction Module - Multi-Provider JSON Extraction
 *
 * Clean extraction pipeline for structured JSON from AI provider outputs.
 * Each provider has different output formats - this module normalizes them.
 *
 * Provider formats:
 * - Claude: {"type":"result","result":{...}} or {"type":"result","structured_output":{...}}
 * - Codex: Raw text in item.created events, turn.completed has NO result field
 * - Gemini: Raw text in message events, result event may have NO result field
 *
 * Extraction priority (most specific → least specific):
 * 1. Result wrapper with content (type:result + result/structured_output field)
 * 2. Accumulated text from provider parser events
 * 3. Markdown code block extraction
 * 4. Direct JSON parse of entire output
 */

const { getProvider, parseChunkWithProvider } = require('../providers');

/**
 * Strip timestamp prefix from log lines.
 * Format: [epochMs]content or [epochMs]{json...}
 *
 * @param {string} line - Raw log line
 * @returns {string} Content without timestamp prefix
 */
function stripTimestamp(line) {
  if (!line || typeof line !== 'string') return '';
  let trimmed = line.trim().replace(/\r$/, '');
  if (!trimmed) return '';

  const tsMatch = trimmed.match(/^\[(\d{13})\](.*)$/);
  if (tsMatch) trimmed = (tsMatch[2] || '').trimStart();

  // In cluster logs, lines are often prefixed like:
  // "validator       | {json...}"
  // Strip the "<agent> | " prefix so we can JSON.parse the event line.
  if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) {
    const pipeMatch = trimmed.match(/^[^|]{1,40}\|\s*(.*)$/);
    if (pipeMatch) {
      const afterPipe = (pipeMatch[1] || '').trimStart();
      if (afterPipe.startsWith('{') || afterPipe.startsWith('[')) return afterPipe;
    }
  }

  return trimmed;
}

/**
 * Strategy 1: Extract from result wrapper
 * Handles Claude CLI format: {"type":"result","result":{...}}
 *
 * @param {string} output - Raw output
 * @returns {object|null} Extracted JSON or null
 */
function extractFromResultWrapper(output) {
  const lines = output.split('\n');

  for (const line of lines) {
    const content = stripTimestamp(line);
    if (!content.startsWith('{')) continue;

    try {
      const obj = JSON.parse(content);
      const extracted = extractResultContent(obj);
      if (extracted) return extracted;
    } catch {
      // Not valid JSON, continue to next line
    }
  }

  return null;
}

function extractResultContent(obj) {
  // Must be type:result WITH actual content
  if (obj?.type !== 'result') return null;

  // Check structured_output first (standard CLI format)
  if (obj.structured_output && typeof obj.structured_output === 'object') {
    return obj.structured_output;
  }

  // Check result field - can be object or string
  if (!obj.result) return null;

  if (typeof obj.result === 'object') {
    return obj.result;
  }

  if (typeof obj.result !== 'string') return null;

  // Result is string - might contain markdown-wrapped JSON
  return extractFromMarkdown(obj.result) || extractDirectJson(obj.result);
}

/**
 * Strategy 2: Extract from accumulated text events
 * Handles non-Claude providers where JSON is in text content
 *
 * @param {string} output - Raw output
 * @param {string} providerName - Provider name for parser selection
 * @returns {object|null} Extracted JSON or null
 */
function extractFromTextEvents(output, providerName) {
  const provider = getProvider(providerName);
  const events = parseChunkWithProvider(provider, output);

  // Accumulate all text events
  const textContent = events
    .filter((e) => e.type === 'text')
    .map((e) => e.text)
    .join('');

  if (!textContent.trim()) return null;

  // Try parsing accumulated text as JSON
  return extractDirectJson(textContent) || extractFromMarkdown(textContent);
}

/**
 * Strategy 3: Extract JSON from markdown code block
 * Handles: ```json\n{...}\n```
 *
 * @param {string} text - Text that may contain markdown
 * @returns {object|null} Extracted JSON or null
 */
function extractFromMarkdown(text) {
  if (!text) return null;

  // Match ```json ... ``` with any whitespace
  const match = text.match(/```json\s*([\s\S]*?)```/);
  if (!match) return null;

  try {
    const parsed = JSON.parse(match[1].trim());
    if (typeof parsed === 'object' && parsed !== null) {
      return parsed;
    }
  } catch {
    // Invalid JSON in markdown block
  }

  return null;
}

/**
 * Strategy 4: Direct JSON parse
 * Handles raw JSON output (single-line or multi-line)
 *
 * @param {string} text - Text to parse
 * @returns {object|null} Parsed JSON or null
 */
function extractDirectJson(text) {
  if (!text) return null;

  const trimmed = text.trim();
  if (!trimmed) return null;

  try {
    const parsed = JSON.parse(trimmed);
    if (typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)) {
      return parsed;
    }
  } catch {
    // Not valid JSON
  }

  return null;
}

/**
 * Main extraction function - tries all strategies in priority order
 *
 * @param {string} output - Raw output from AI provider CLI
 * @param {string} providerName - Provider name ('claude', 'codex', 'gemini')
 * @returns {object|null} Extracted JSON object or null if extraction failed
 */
function extractJsonFromOutput(output, providerName = 'claude') {
  if (!output || typeof output !== 'string') return null;

  const trimmedOutput = output.trim();
  if (!trimmedOutput) return null;

  // Check for fatal error indicators
  if (trimmedOutput.includes('Task not found') || trimmedOutput.includes('Process terminated')) {
    return null;
  }

  // Strategy 1: Result wrapper (Claude format)
  const fromWrapper = extractFromResultWrapper(trimmedOutput);
  if (fromWrapper) return fromWrapper;

  // Strategy 2: Text events (non-Claude providers)
  const fromText = extractFromTextEvents(trimmedOutput, providerName);
  if (fromText) return fromText;

  // Strategy 3: Markdown extraction
  const fromMarkdown = extractFromMarkdown(trimmedOutput);
  if (fromMarkdown) return fromMarkdown;

  // Strategy 4: Direct JSON parse (raw output)
  const fromDirect = extractDirectJson(trimmedOutput);
  if (fromDirect) return fromDirect;

  return null;
}

module.exports = {
  extractJsonFromOutput,
  extractFromResultWrapper,
  extractFromTextEvents,
  extractFromMarkdown,
  extractDirectJson,
  stripTimestamp,
};
