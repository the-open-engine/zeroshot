import {
  getArray,
  getNumber,
  getOptionalString,
  getRecord,
  getString,
  isRecord,
  stringifyJson,
  tryParseJson,
} from '../json';
import type { OutputEvent, ProviderParseResult, ResultEvent } from '../types';

function cacheReadInputTokens(usage: Record<string, unknown>): number {
  return (
    getNumber(usage, 'cache_read_input_tokens') ?? getNumber(usage, 'cached_input_tokens') ?? 0
  );
}

function parseResultEvent(event: Record<string, unknown>): OutputEvent {
  const usage = getRecord(event, 'usage') ?? {};
  const result: ResultEvent = {
    type: 'result',
    success: getString(event, 'subtype') === 'success',
    result: event.result,
    error: event.is_error ? event.result : null,
    cost: event.total_cost_usd,
    duration: event.duration_ms,
    inputTokens: getNumber(usage, 'input_tokens') ?? 0,
    outputTokens: getNumber(usage, 'output_tokens') ?? 0,
    cacheReadInputTokens: cacheReadInputTokens(usage),
    cacheCreationInputTokens: getNumber(usage, 'cache_creation_input_tokens') ?? 0,
    modelUsage: event.modelUsage ?? null,
  };
  return result;
}

function parseStreamEvent(inner: Record<string, unknown>): OutputEvent | null {
  if (getString(inner, 'type') !== 'content_block_delta') return null;
  const delta = getRecord(inner, 'delta');
  if (delta === null) return null;

  if (getString(delta, 'type') === 'text_delta') {
    const text = getString(delta, 'text');
    return text ? { type: 'text', text } : null;
  }

  if (getString(delta, 'type') === 'thinking_delta') {
    const text = getString(delta, 'thinking');
    return text ? { type: 'thinking', text } : null;
  }

  return null;
}

function parseAssistantBlock(block: Record<string, unknown>): OutputEvent | null {
  const blockType = getString(block, 'type');
  if (blockType === 'text') {
    const text = getString(block, 'text');
    return text ? { type: 'text', text } : null;
  }
  if (blockType === 'tool_use') {
    return {
      type: 'tool_call',
      toolName: getOptionalString(block, 'name'),
      toolId: getOptionalString(block, 'id'),
      input: block.input,
    };
  }
  if (blockType === 'thinking') {
    const text = getString(block, 'thinking');
    return text ? { type: 'thinking', text } : null;
  }
  return null;
}

function parseEventList(events: readonly OutputEvent[]): ProviderParseResult {
  if (events.length === 1) return events[0] ?? null;
  if (events.length > 1) return events;
  return null;
}

function parseAssistantMessage(message: Record<string, unknown>): ProviderParseResult {
  const results: OutputEvent[] = [];
  for (const blockValue of getArray(message, 'content')) {
    if (!isRecord(blockValue)) continue;
    const event = parseAssistantBlock(blockValue);
    if (event !== null) results.push(event);
  }
  return parseEventList(results);
}

function parseUserBlock(block: Record<string, unknown>): OutputEvent | null {
  if (getString(block, 'type') !== 'tool_result') return null;
  const content = block.content;
  return {
    type: 'tool_result',
    toolId: getOptionalString(block, 'tool_use_id'),
    content:
      typeof content === 'string' || content === undefined ? content : stringifyJson(content),
    isError: block.is_error || false,
  };
}

function parseUserMessage(message: Record<string, unknown>): ProviderParseResult {
  const results: OutputEvent[] = [];
  for (const blockValue of getArray(message, 'content')) {
    if (!isRecord(blockValue)) continue;
    const event = parseUserBlock(blockValue);
    if (event !== null) results.push(event);
  }
  return parseEventList(results);
}

export function parseClaudeEvent(line: string): ProviderParseResult {
  const parsed = tryParseJson(line.trim());
  if (!isRecord(parsed)) return null;

  const eventType = getString(parsed, 'type');
  if (eventType === 'stream_event') {
    const inner = getRecord(parsed, 'event');
    return inner === null ? null : parseStreamEvent(inner);
  }
  if (eventType === 'assistant') {
    const message = getRecord(parsed, 'message');
    return message === null ? null : parseAssistantMessage(message);
  }
  if (eventType === 'user') {
    const message = getRecord(parsed, 'message');
    return message === null ? null : parseUserMessage(message);
  }
  if (eventType === 'result') return parseResultEvent(parsed);
  return null;
}
