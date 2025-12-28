/**
 * Stream JSON Parser for Claude Code output
 *
 * Parses NDJSON (newline-delimited JSON) streaming output from Claude Code.
 * Extracts: text output, tool calls, tool results, thinking, errors.
 *
 * Event types from Claude Code stream-json format:
 * - system: Session initialization
 * - stream_event: Real-time streaming (content_block_start, content_block_delta, etc.)
 * - assistant: Complete assistant message with content array
 * - user: Tool results
 * - result: Final task result
 */

/**
 * Parse a single JSON line and extract displayable content
 * @param {string} line - Single line of NDJSON
 * @returns {Object|null} Parsed event with type and content, or null if not displayable
 */
function parseStreamLine(line) {
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

  // stream_event - real-time streaming updates
  if (event.type === 'stream_event' && event.event) {
    return parseStreamEvent(event.event);
  }

  // assistant - complete message with content blocks
  if (event.type === 'assistant' && event.message?.content) {
    return parseAssistantMessage(event.message);
  }

  // user - tool result
  if (event.type === 'user' && event.message?.content) {
    return parseUserMessage(event.message);
  }

  // result - final task result (includes token usage and cost)
  if (event.type === 'result') {
    const usage = event.usage || {};
    return {
      type: 'result',
      success: event.subtype === 'success',
      result: event.result,
      error: event.is_error ? event.result : null,
      cost: event.total_cost_usd,
      duration: event.duration_ms,
      // Token usage from Claude API
      inputTokens: usage.input_tokens || 0,
      outputTokens: usage.output_tokens || 0,
      cacheReadInputTokens: usage.cache_read_input_tokens || 0,
      cacheCreationInputTokens: usage.cache_creation_input_tokens || 0,
      // Per-model breakdown (for multi-model tasks)
      modelUsage: event.modelUsage || null,
    };
  }

  // system - session init (skip, not user-facing)
  if (event.type === 'system') {
    return null;
  }

  return null;
}

/**
 * Parse stream_event inner event
 */
function parseStreamEvent(inner) {
  // content_block_start - tool use or text block starting
  if (inner.type === 'content_block_start' && inner.content_block) {
    const block = inner.content_block;

    if (block.type === 'tool_use') {
      return {
        type: 'tool_start',
        toolName: block.name,
        toolId: block.id,
      };
    }

    if (block.type === 'thinking') {
      return {
        type: 'thinking_start',
      };
    }

    // text block start - usually empty, skip
    return null;
  }

  // content_block_delta - incremental content
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

    if (delta.type === 'input_json_delta' && delta.partial_json) {
      return {
        type: 'tool_input',
        json: delta.partial_json,
      };
    }
  }

  // content_block_stop - block ended
  if (inner.type === 'content_block_stop') {
    return {
      type: 'block_end',
      index: inner.index,
    };
  }

  // message_start - skip text extraction here since it will come via text_delta events
  // (extracting here causes duplicate text output)

  return null;
}

/**
 * Parse assistant message content blocks
 * NOTE: Skip text blocks here since they were already streamed via text_delta events.
 * Extracting text here would cause duplicate output.
 */
function parseAssistantMessage(message) {
  const results = [];

  for (const block of message.content) {
    // Skip text blocks - already streamed via text_delta events
    // Extracting here causes duplicate output

    if (block.type === 'tool_use') {
      results.push({
        type: 'tool_call',
        toolName: block.name,
        toolId: block.id,
        input: block.input,
      });
    }

    if (block.type === 'thinking' && block.thinking) {
      // Skip thinking blocks too - already streamed via thinking_delta
      // But keep for non-streaming contexts (direct API responses)
      // Only emit if we haven't seen streaming deltas (detected by having results already)
    }
  }

  if (results.length === 1) {
    return results[0];
  }

  if (results.length > 1) {
    return {
      type: 'multi',
      events: results,
    };
  }

  return null;
}

/**
 * Parse user message (tool results)
 */
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
    return {
      type: 'multi',
      events: results,
    };
  }

  return null;
}

/**
 * Parse multiple lines of NDJSON
 * @param {string} chunk - Chunk of text potentially containing multiple JSON lines
 * @returns {Array} Array of parsed events
 */
function parseChunk(chunk) {
  const events = [];
  const lines = chunk.split('\n');

  for (const line of lines) {
    const event = parseStreamLine(line);
    if (event) {
      if (event.type === 'multi') {
        events.push(...event.events);
      } else {
        events.push(event);
      }
    }
  }

  return events;
}

module.exports = {
  parseStreamLine,
  parseChunk,
  parseStreamEvent,
  parseAssistantMessage,
  parseUserMessage,
};
