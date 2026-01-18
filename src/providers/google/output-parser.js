function normalizeMessageContent(content) {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    return content.map((item) => (typeof item === 'string' ? item : item?.text || '')).join('');
  }
  if (content && typeof content === 'object') {
    if (typeof content.text === 'string') return content.text;
  }
  return '';
}

function parseMessageEvent(event) {
  if (event.role === 'assistant') {
    const text = normalizeMessageContent(event.content);
    if (text) {
      return { type: 'text', text };
    }
  }
  return null;
}

function parseToolUseEvent(event, state) {
  const toolId = event.tool_call_id || event.tool_id || event.id || state.lastToolId;
  const toolName = event.tool_name || event.name;
  state.lastToolId = toolId;
  return {
    type: 'tool_call',
    toolName,
    toolId,
    input: event.parameters || event.input || {},
  };
}

function parseToolResultEvent(event, state) {
  const toolId = event.tool_call_id || event.tool_id || event.id || state.lastToolId;
  return {
    type: 'tool_result',
    toolId,
    content: event.output ?? event.content ?? '',
    isError: event.success === false,
  };
}

function parseResultEvent(event) {
  return {
    type: 'result',
    success: event.success !== false,
    result: event.result || '',
    error: event.success === false ? event.error || 'Result failed' : null,
  };
}

function parseEvent(line, state = {}, options = {}) {
  let event;
  try {
    event = JSON.parse(line);
  } catch {
    return null;
  }

  switch (event.type) {
    case 'init':
      return null;
    case 'message':
      return parseMessageEvent(event);
    case 'tool_use':
      return parseToolUseEvent(event, state);
    case 'tool_result':
      return parseToolResultEvent(event, state);
    case 'result':
      return parseResultEvent(event);
    default:
      if (options.onUnknown) {
        options.onUnknown(event.type, event);
      }
      return null;
  }
}

function parseChunk(chunk, state = {}, options = {}) {
  const events = [];
  const lines = chunk.split('\n');

  for (const line of lines) {
    if (!line.trim()) continue;
    const event = parseEvent(line, state, options);
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
