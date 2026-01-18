/**
 * Stream JSON Parser for Claude Code output
 */

/**
 * Parse result event type
 * @param {Object} event
 * @returns {Object}
 */
function parseResultEvent(event) {
  const usage = event.usage || {};
  return {
    type: 'result',
    success: event.subtype === 'success',
    result: event.result,
    error: event.is_error ? event.result : null,
    cost: event.total_cost_usd,
    duration: event.duration_ms,
    inputTokens: usage.input_tokens || 0,
    outputTokens: usage.output_tokens || 0,
    cacheReadInputTokens: usage.cache_read_input_tokens || 0,
    cacheCreationInputTokens: usage.cache_creation_input_tokens || 0,
    modelUsage: event.modelUsage || null,
  };
}

/**
 * Parse a single JSON line and extract displayable content
 * @param {string} line
 * @returns {Object|null}
 */
function parseEvent(line) {
  const trimmed = line.trim();
  if (!trimmed || !trimmed.startsWith('{') || !trimmed.endsWith('}')) {
    return null;
  }

  let event;
  try {
    event = JSON.parse(trimmed);
  } catch {
    return null;
  }

  if (event.type === 'stream_event' && event.event) {
    return parseStreamEvent(event.event);
  }

  if (event.type === 'assistant' && event.message?.content) {
    return parseAssistantMessage(event.message);
  }

  if (event.type === 'user' && event.message?.content) {
    return parseUserMessage(event.message);
  }

  if (event.type === 'result') {
    return parseResultEvent(event);
  }

  if (event.type === 'system') {
    return null;
  }

  return null;
}

function parseStreamEvent(inner) {
  if (inner.type === 'content_block_delta' && inner.delta) {
    const delta = inner.delta;

    if (delta.type === 'text_delta' && delta.text) {
      return {
        type: 'text',
        text: delta.text,
      };
    }

    if (delta.type === 'thinking_delta' && delta.thinking) {
      return {
        type: 'thinking',
        text: delta.thinking,
      };
    }
  }

  return null;
}

function parseAssistantMessage(message) {
  const results = [];

  for (const block of message.content) {
    // Handle text content blocks (CRITICAL for Haiku/weaker models that return JSON in text)
    // Issue #52: Haiku returns JSON in type:text blocks, not in result wrapper
    if (block.type === 'text' && block.text) {
      results.push({
        type: 'text',
        text: block.text,
      });
    }

    if (block.type === 'tool_use') {
      results.push({
        type: 'tool_call',
        toolName: block.name,
        toolId: block.id,
        input: block.input,
      });
    }

    if (block.type === 'thinking' && block.thinking) {
      results.push({
        type: 'thinking',
        text: block.thinking,
      });
    }
  }

  if (results.length === 1) {
    return results[0];
  }

  if (results.length > 1) {
    return results;
  }

  return null;
}

function parseUserMessage(message) {
  const results = [];

  for (const block of message.content) {
    if (block.type === 'tool_result') {
      results.push({
        type: 'tool_result',
        toolId: block.tool_use_id,
        content: typeof block.content === 'string' ? block.content : JSON.stringify(block.content),
        isError: block.is_error || false,
      });
    }
  }

  if (results.length === 1) {
    return results[0];
  }

  if (results.length > 1) {
    return results;
  }

  return null;
}

function parseChunk(chunk) {
  const events = [];
  const lines = chunk.split('\n');

  for (const line of lines) {
    const event = parseEvent(line);
    if (event) {
      if (Array.isArray(event)) {
        events.push(...event);
      } else {
        events.push(event);
      }
    }
  }

  return events;
}

module.exports = {
  parseEvent,
  parseChunk,
};
