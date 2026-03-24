/**
 * MiniMax Output Parser
 *
 * Parses JSON events from the MiniMax CLI wrapper (cli-wrapper.js).
 * The wrapper outputs OpenAI-compatible streaming events normalized
 * to the zeroshot event format.
 */

function parseEvent(line) {
  let event;
  try {
    event = JSON.parse(line);
  } catch {
    return null;
  }

  if (!event || typeof event !== 'object') return null;

  switch (event.type) {
    case 'text':
      return event.text ? { type: 'text', text: event.text } : null;

    case 'thinking':
      return event.text ? { type: 'thinking', text: event.text } : null;

    case 'tool_call':
      return {
        type: 'tool_call',
        toolName: event.toolName || event.name,
        toolId: event.toolId || event.id,
        input: event.input || {},
      };

    case 'tool_result':
      return {
        type: 'tool_result',
        toolId: event.toolId || event.id,
        content: event.content || '',
        isError: !!event.isError,
      };

    case 'result':
      return {
        type: 'result',
        success: event.success !== false,
        inputTokens: event.inputTokens || 0,
        outputTokens: event.outputTokens || 0,
        error: event.error || null,
      };

    case 'error':
      return {
        type: 'result',
        success: false,
        error: event.error || event.message || 'Unknown MiniMax error',
      };

    default:
      return null;
  }
}

function parseChunk(chunk) {
  const events = [];
  const lines = chunk.split('\n');

  for (const line of lines) {
    if (!line.trim()) continue;
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
