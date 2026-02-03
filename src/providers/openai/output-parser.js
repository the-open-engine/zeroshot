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

function parseCommandExecutionItem(item, phase) {
  // Codex CLI (newer) emits `command_execution` items for bash-like tool runs.
  // Map them into the shared schema expected by the logs renderer.
  const toolId = item.id;
  const command = item.command || item.cmd || item.input?.command || item.input?.cmd;

  if (phase === 'started') {
    return {
      type: 'tool_call',
      toolName: 'Bash',
      toolId,
      input: command ? { command } : {},
    };
  }

  const output = item.aggregated_output ?? item.output ?? item.result ?? '';
  const exitCode =
    typeof item.exit_code === 'number'
      ? item.exit_code
      : typeof item.exitCode === 'number'
        ? item.exitCode
        : null;

  return {
    type: 'tool_result',
    toolId,
    content: output,
    isError: exitCode !== null ? exitCode !== 0 : !!item.error,
  };
}

function parseReasoningItem(item) {
  const text = item.text || item.content || '';
  if (!text) return null;
  return { type: 'thinking', text };
}

function parseItem(item, eventType) {
  const events = [];
  const phase =
    eventType === 'item.started' ? 'started' : eventType === 'item.completed' ? 'completed' : null;

  // Handle assistant messages (Claude-style: type=message, role=assistant)
  if (item.type === 'message' && item.role === 'assistant') {
    events.push(...parseAssistantMessage(item));
  }

  // Handle agent messages (Codex-style: type=agent_message, text=string)
  if (item.type === 'agent_message' && item.text) {
    events.push({ type: 'text', text: item.text });
  }

  if (item.type === 'reasoning') {
    const reasoning = parseReasoningItem(item);
    if (reasoning) events.push(reasoning);
  }

  if (item.type === 'command_execution' && phase) {
    events.push(parseCommandExecutionItem(item, phase));
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
    case 'error':
      return {
        type: 'result',
        success: false,
        error: event.error?.message || event.message || event.error || 'Error',
      };

    case 'thread.started':
    case 'turn.started':
      return null;

    case 'item.started':
      if (!event.item || event.item.type !== 'command_execution') return null;
      return parseItem(event.item, event.type);

    case 'item.created':
    case 'item.completed':
      if (!event.item) return null;
      return parseItem(event.item, event.type);

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
