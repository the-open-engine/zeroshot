function parseToolPart(part) {
  const state = part.state || {};

  if (state.status === 'pending' || state.status === 'running') {
    return {
      type: 'tool_call',
      toolName: part.tool,
      toolId: part.callID,
      input: state.input || {},
    };
  }

  if (state.status === 'completed') {
    return {
      type: 'tool_result',
      toolId: part.callID,
      content: state.output || '',
      isError: false,
    };
  }

  if (state.status === 'error') {
    return {
      type: 'tool_result',
      toolId: part.callID,
      content: state.error || '',
      isError: true,
    };
  }

  return null;
}

function parseStepFinish(part) {
  const tokens = part.tokens || {};
  return {
    type: 'result',
    success: true,
    inputTokens: tokens.input || 0,
    outputTokens: tokens.output || 0,
  };
}

function parsePart(part) {
  if (!part || typeof part !== 'object') return null;

  if (part.type === 'text' && part.text) {
    return { type: 'text', text: part.text };
  }

  if (part.type === 'reasoning' && part.text) {
    return { type: 'thinking', text: part.text };
  }

  if (part.type === 'tool') {
    return parseToolPart(part);
  }

  if (part.type === 'step-finish') {
    return parseStepFinish(part);
  }

  return null;
}

function parseErrorEvent(event) {
  const error = event.error || {};
  return {
    type: 'result',
    success: false,
    error: error.data?.message || error.message || error.name || 'Unknown error',
  };
}

function parseEvent(line) {
  let event;
  try {
    event = JSON.parse(line);
  } catch {
    return null;
  }

  if (!event || typeof event !== 'object') return null;

  if (event.type === 'error') {
    return parseErrorEvent(event);
  }

  if (event.type === 'text' || event.type === 'step_start' || event.type === 'step_finish') {
    return parsePart(event.part || event);
  }

  if (event.type === 'message.part.updated') {
    return parsePart(event.properties?.part);
  }

  return null;
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
  parsePart,
};
