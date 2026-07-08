import {
  getNumber,
  getOptionalString,
  getOrStringFromKeys,
  getRecord,
  getString,
  isRecord,
  parseJson,
  tryParseJson,
} from '../json';
import type { OutputEvent, ProviderParseResult } from '../types';

function safeJsonParse(value: string, fallback: unknown): unknown {
  try {
    return parseJson(value);
  } catch {
    return fallback;
  }
}

function parseAssistantMessage(item: Record<string, unknown>): OutputEvent[] {
  const rawContent = item.content;
  const content = Array.isArray(rawContent) ? rawContent : [{ type: 'text', text: rawContent }];
  const records = content.filter(isRecord);
  const text = records
    .filter((contentItem) => getString(contentItem, 'type') === 'text')
    .map((contentItem) => getString(contentItem, 'text') ?? '')
    .join('');
  const thinking = records
    .filter((contentItem) => {
      const type = getString(contentItem, 'type');
      return type === 'thinking' || type === 'reasoning';
    })
    .map((contentItem) => getString(contentItem, 'text') ?? '')
    .join('');

  const events: OutputEvent[] = [];
  if (text) events.push({ type: 'text', text });
  if (thinking) events.push({ type: 'thinking', text: thinking });
  return events;
}

function parseFunctionCall(item: Record<string, unknown>): OutputEvent {
  const argumentValue = item.arguments;
  const input =
    typeof argumentValue === 'string' ? safeJsonParse(argumentValue, {}) : (argumentValue ?? {});
  return {
    type: 'tool_call',
    toolName: getOptionalString(item, 'name'),
    toolId: getOrStringFromKeys(item, ['call_id', 'id', 'tool_call_id', 'tool_id']),
    input,
  };
}

function parseFunctionCallOutput(item: Record<string, unknown>): OutputEvent {
  return {
    type: 'tool_result',
    toolId: getOrStringFromKeys(item, ['call_id', 'id', 'tool_call_id', 'tool_id']),
    content: item.output ?? item.result ?? item.content ?? '',
    isError: Boolean(item.error),
  };
}

function commandFromItem(item: Record<string, unknown>): string | null {
  const input = getRecord(item, 'input');
  if (input !== null) {
    return (
      getString(item, 'command') ??
      getString(item, 'cmd') ??
      getString(input, 'command') ??
      getString(input, 'cmd')
    );
  }
  return getString(item, 'command') ?? getString(item, 'cmd');
}

function commandExecutionResultIsError(item: Record<string, unknown>): boolean {
  const exitCode = getNumber(item, 'exit_code') ?? getNumber(item, 'exitCode');
  if (exitCode !== null) return exitCode !== 0;
  return Boolean(item.error);
}

function parseCommandExecutionItem(
  item: Record<string, unknown>,
  phase: 'started' | 'completed'
): OutputEvent {
  const command = commandFromItem(item);
  const toolId = getOptionalString(item, 'id');

  if (phase === 'started') {
    return {
      type: 'tool_call',
      toolName: 'Bash',
      toolId,
      input: command ? { command } : {},
    };
  }

  return {
    type: 'tool_result',
    toolId,
    content: item.aggregated_output ?? item.output ?? item.result ?? '',
    isError: commandExecutionResultIsError(item),
  };
}

function parseReasoningItem(item: Record<string, unknown>): OutputEvent | null {
  const text = getString(item, 'text') ?? getString(item, 'content') ?? '';
  return text ? { type: 'thinking', text } : null;
}

function normalizeItemEvents(events: OutputEvent[]): ProviderParseResult {
  if (events.length === 1) return events[0] ?? null;
  if (events.length > 1) return events;
  return null;
}

function phaseForEventType(eventType: string): 'started' | 'completed' | null {
  if (eventType === 'item.started') return 'started';
  if (eventType === 'item.completed') return 'completed';
  return null;
}

function pushMaybe(events: OutputEvent[], event: OutputEvent | null): void {
  if (event !== null) events.push(event);
}

function parseItem(item: Record<string, unknown>, eventType: string): ProviderParseResult {
  const events: OutputEvent[] = [];
  const itemType = getString(item, 'type');
  const phase = phaseForEventType(eventType);

  if (itemType === 'message' && getString(item, 'role') === 'assistant') {
    events.push(...parseAssistantMessage(item));
  }
  if (itemType === 'agent_message') {
    const text = getString(item, 'text');
    if (text) events.push({ type: 'text', text });
  }
  if (itemType === 'reasoning') pushMaybe(events, parseReasoningItem(item));
  if (itemType === 'command_execution' && phase !== null) {
    events.push(parseCommandExecutionItem(item, phase));
  }
  if (itemType === 'function_call') events.push(parseFunctionCall(item));
  if (itemType === 'function_call_output') events.push(parseFunctionCallOutput(item));

  return normalizeItemEvents(events);
}

function parseErrorEvent(event: Record<string, unknown>): OutputEvent {
  const error = getRecord(event, 'error');
  return {
    type: 'result',
    success: false,
    error: error
      ? (getString(error, 'message') ?? getString(event, 'message') ?? event.error ?? 'Error')
      : (getString(event, 'message') ?? event.error ?? 'Error'),
  };
}

function parseTurnCompleted(event: Record<string, unknown>): OutputEvent {
  const response = getRecord(event, 'response');
  const usage = getRecord(event, 'usage') ?? (response ? getRecord(response, 'usage') : null) ?? {};
  return {
    type: 'result',
    success: true,
    inputTokens: getNumber(usage, 'input_tokens') ?? 0,
    outputTokens: getNumber(usage, 'output_tokens') ?? 0,
  };
}

function parseTurnFailed(event: Record<string, unknown>): OutputEvent {
  const error = getRecord(event, 'error');
  return {
    type: 'result',
    success: false,
    error: error
      ? (getString(error, 'message') ?? event.error ?? 'Turn failed')
      : (event.error ?? 'Turn failed'),
  };
}

function parseStartedItem(event: Record<string, unknown>): ProviderParseResult {
  const item = getRecord(event, 'item');
  if (item === null || getString(item, 'type') !== 'command_execution') return null;
  return parseItem(item, 'item.started');
}

function parseCreatedOrCompletedItem(
  event: Record<string, unknown>,
  type: string
): ProviderParseResult {
  const item = getRecord(event, 'item');
  return item === null ? null : parseItem(item, type);
}

export function parseCodexEvent(line: string): ProviderParseResult {
  const event = tryParseJson(line);
  if (!isRecord(event)) return null;

  const type = getString(event, 'type');
  if (type === 'error') return parseErrorEvent(event);
  if (type === 'thread.started' || type === 'turn.started') return null;
  if (type === 'item.started') return parseStartedItem(event);
  if (type === 'item.created' || type === 'item.completed') {
    return parseCreatedOrCompletedItem(event, type);
  }
  if (type === 'turn.completed') return parseTurnCompleted(event);
  if (type === 'turn.failed') return parseTurnFailed(event);
  return null;
}
