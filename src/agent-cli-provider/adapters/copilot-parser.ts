// Copilot `--output-format json` emits JSONL; payload under `data`, dot-namespaced `type` (verified
// v1.0.69). Unknown types are ignored (fail-open). Mapping:
//   assistant.message_delta/message → text, or thinking when phase==='commentary' (final answer is
//     the result text). assistant.reasoning → thinking. tool.execution_start → tool_call.
//   tool.execution_complete → tool_result. result → terminal (success = top-level exitCode === 0).
import { getBoolean, getNumber, getRecord, getString, isRecord, tryParseJson } from '../json';
import type { OutputEvent, ProviderParseResult, ProviderParserState } from '../types';

const IGNORED_TYPES = new Set([
  'user.message',
  'assistant.turn_start',
  'assistant.turn_end',
  'assistant.idle',
  'assistant.tool_call_delta',
  'tool.execution_partial_result',
]);

function messageTextMap(state: ProviderParserState): Map<string, string> {
  if (!state.assistantTextByMessageId) state.assistantTextByMessageId = new Map();
  return state.assistantTextByMessageId;
}

function messagePhaseMap(state: ProviderParserState): Map<string, string> {
  if (!state.messagePhaseById) state.messagePhaseById = new Map();
  return state.messagePhaseById;
}

// `commentary` = the model narrating its plan (thinking); anything else is user-facing output.
function isCommentaryPhase(phase: string | null | undefined): boolean {
  return phase === 'commentary';
}

function snapshotDelta(previous: string, current: string): string | null {
  if (!current) return null;
  if (previous === current) return null;
  if (current.startsWith(previous)) return current.slice(previous.length) || null;
  return current;
}

function accrueOutputTokens(state: ProviderParserState, data: Record<string, unknown>): void {
  const tokens = getNumber(data, 'outputTokens');
  if (tokens === null) return;
  const usage = state.usage ?? {};
  state.usage = { ...usage, outputTokens: (usage.outputTokens ?? 0) + tokens };
}

function parseMessageStart(data: Record<string, unknown>, state: ProviderParserState): null {
  const messageId = getString(data, 'messageId');
  if (messageId === null) return null;
  messageTextMap(state).set(messageId, '');
  const phase = getString(data, 'phase');
  if (phase !== null) messagePhaseMap(state).set(messageId, phase);
  return null;
}

function textOrThinking(commentary: boolean, text: string): OutputEvent {
  return commentary ? { type: 'thinking', text } : { type: 'text', text };
}

function parseMessageDelta(
  data: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent | null {
  const delta = getString(data, 'deltaContent');
  if (!delta) return null;
  const messageId = getString(data, 'messageId') ?? '';
  const map = messageTextMap(state);
  map.set(messageId, (map.get(messageId) ?? '') + delta);
  return textOrThinking(isCommentaryPhase(messagePhaseMap(state).get(messageId)), delta);
}

function parseMessage(
  data: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent | null {
  const content = getString(data, 'content') ?? '';
  accrueOutputTokens(state, data);
  const messageId = getString(data, 'messageId') ?? '';
  const map = messageTextMap(state);
  const delta = snapshotDelta(map.get(messageId) ?? '', content);
  map.set(messageId, content);
  const commentary = isCommentaryPhase(getString(data, 'phase') ?? messagePhaseMap(state).get(messageId));
  // Only the final answer (not commentary narration) is the run's result text.
  if (content && !commentary) state.lastAssistantText = content;
  return delta ? textOrThinking(commentary, delta) : null;
}

function parseReasoning(data: Record<string, unknown>): OutputEvent | null {
  const text = getString(data, 'content');
  return text ? { type: 'thinking', text } : null;
}

function parseToolExecutionStart(
  data: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent {
  const toolId = getString(data, 'toolCallId');
  state.lastToolId = toolId;
  return {
    type: 'tool_call',
    toolName: getString(data, 'toolName'),
    toolId,
    input: Object.prototype.hasOwnProperty.call(data, 'arguments') ? data.arguments : {},
  };
}

function toolResultContent(data: Record<string, unknown>): unknown {
  const result = getRecord(data, 'result');
  if (result !== null) return result.content ?? result.detailedContent ?? '';
  return data.result ?? '';
}

function parseToolExecutionComplete(
  data: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent {
  const success = getBoolean(data, 'success');
  return {
    type: 'tool_result',
    toolId: getString(data, 'toolCallId') ?? state.lastToolId,
    content: toolResultContent(data),
    isError: success === false,
  };
}

function parseResult(event: Record<string, unknown>, state: ProviderParserState): OutputEvent {
  const exitCode = getNumber(event, 'exitCode') ?? 0;
  const success = exitCode === 0;
  const usage = state.usage ?? {};
  return {
    type: 'result',
    success,
    result: success ? (state.lastAssistantText ?? null) : null,
    error: success ? null : `Copilot exited with code ${exitCode}`,
    inputTokens: usage.inputTokens ?? 0,
    outputTokens: usage.outputTokens ?? 0,
    cacheReadInputTokens: usage.cacheReadInputTokens ?? 0,
    cacheCreationInputTokens: usage.cacheCreationInputTokens ?? 0,
  };
}

export function parseCopilotEvent(line: string, state: ProviderParserState): ProviderParseResult {
  const event = tryParseJson(line);
  if (!isRecord(event)) return null;

  const type = getString(event, 'type');
  if (type === null || type.startsWith('session.') || IGNORED_TYPES.has(type)) return null;

  if (type === 'result') return parseResult(event, state);

  const data = getRecord(event, 'data') ?? {};
  switch (type) {
    case 'assistant.message_start':
      return parseMessageStart(data, state);
    case 'assistant.message_delta':
      return parseMessageDelta(data, state);
    case 'assistant.message':
      return parseMessage(data, state);
    case 'assistant.reasoning':
      return parseReasoning(data);
    case 'tool.execution_start':
      return parseToolExecutionStart(data, state);
    case 'tool.execution_complete':
      return parseToolExecutionComplete(data, state);
    default:
      return null;
  }
}
