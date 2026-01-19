function safeJsonParse(value, fallback) {
  try {
    return JSON.parse(value);
  } catch {
    return fallback;
  }
}

function parseAssistantMessage(item) {
  const events = [];
  const content = Array.isArray(item.content)
    ? item.content
    : [{ type: 'text', text: item.content }];
  const text = content
    .filter((c) => c.type === 'text')
    .map((c) => c.text)
    .join('');
  const thinking = content
    .filter((c) => c.type === 'thinking' || c.type === 'reasoning')
    .map((c) => c.text)
    .join('');
  if (text) events.push({ type: 'text', text });
  if (thinking) events.push({ type: 'thinking', text: thinking });
  return events;
}

function parseFunctionCall(item) {
  const toolId = item.call_id || item.id || item.tool_call_id || item.tool_id;
  const args =
    typeof item.arguments === 'string' ? safeJsonParse(item.arguments, {}) : item.arguments || {};
  return {
    type: 'tool_call',
    toolName: item.name,
    toolId,
    input: args,
  };
}

function parseFunctionCallOutput(item) {
  const toolId = item.call_id || item.id || item.tool_call_id || item.tool_id;
  const content = item.output ?? item.result ?? item.content ?? '';
  return {
    type: 'tool_result',
    toolId,
    content,
    isError: !!item.error,
  };
}

function parseItem(item) {
  const events = [];

  // Handle assistant messages (Claude-style: type=message, role=assistant)
  if (item.type === 'message' && item.role === 'assistant') {
    events.push(...parseAssistantMessage(item));
  }

  // Handle agent messages (Codex-style: type=agent_message, text=string)
  if (item.type === 'agent_message' && item.text) {
    events.push({ type: 'text', text: item.text });
  }

  if (item.type === 'function_call') {
    events.push(parseFunctionCall(item));
  }

  if (item.type === 'function_call_output') {
    events.push(parseFunctionCallOutput(item));
  }

  if (events.length === 1) return events[0];
  if (events.length > 1) return events;
  return null;
}

function parseEvent(line, options = {}) {
  let event;
  try {
    event = JSON.parse(line);
  } catch {
    return null;
  }

  switch (event.type) {
    case 'thread.started':
    case 'turn.started':
      return null;

    case 'item.created':
    case 'item.completed':
      return parseItem(event.item);

    case 'turn.completed': {
      const usage = event.usage || event.response?.usage || {};
      return {
        type: 'result',
        success: true,
        inputTokens: usage.input_tokens || 0,
        outputTokens: usage.output_tokens || 0,
      };
    }

    case 'turn.failed':
      return {
        type: 'result',
        success: false,
        error: event.error?.message || event.error || 'Turn failed',
      };

    default:
      // Only log warnings for actual unknown string types, not malformed events
      // (undefined/object types are just noise from non-standard CLI output)
      if (options.onUnknown && typeof event.type === 'string') {
        options.onUnknown(event.type, event);
      }
      return null;
  }
}

function parseChunk(chunk, options = {}) {
  const events = [];
  const lines = chunk.split('\n');

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
  parseItem,
  safeJsonParse,
};
