import { appendJsonSchemaPrompt } from '../schema';
import { contractError } from '../contract-errors';
import {
  getArray,
  getBoolean,
  getNumber,
  getOptionalString,
  getRecord,
  getString,
  isRecord,
  tryParseJson,
  unknownToMessage,
} from '../json';
import {
  type BuildProviderCommandOptions,
  type CommandSpec,
  type ErrorClassification,
  type LevelModelSpec,
  type LevelOverrides,
  type ModelCatalogEntry,
  type ModelLevel,
  type OutputEvent,
  type PiCliFeatures,
  type ProviderAdapter,
  type ProviderParserState,
  type ResolvedModelSpec,
  type WarningMetadata,
} from '../types';
import {
  classifyBaseProviderError,
  commandSpec,
  createParserState,
  optionFeatures,
  resolveModelSpecWithConfig,
  warning,
} from './common';

const MODEL_CATALOG: Readonly<Record<string, ModelCatalogEntry>> = {};

const LEVEL_MAPPING: Readonly<Record<ModelLevel, LevelModelSpec>> = {
  level1: { rank: 1, model: null },
  level2: { rank: 2, model: null },
  level3: { rank: 3, model: null },
};

const IGNORED_EVENT_TYPES = new Set([
  'session',
  'agent_start',
  'agent_end',
  'turn_start',
  'queue_update',
  'compaction_start',
  'compaction_end',
  'auto_retry_start',
  'auto_retry_end',
]);

function detectCliFeatures(helpText?: string | null): PiCliFeatures {
  const help = helpText ?? '';
  const unknown = !help;
  return {
    provider: 'pi',
    supportsJsonMode: unknown ? true : /--mode\b/.test(help) && /\bjson\b/.test(help),
    supportsModel: unknown ? true : /--model\b/.test(help),
    supportsNoSession: unknown ? true : /--no-session\b/.test(help),
    supportsNoExtensions: unknown ? true : /--no-extensions\b/.test(help),
    supportsNoSkills: unknown ? true : /--no-skills\b/.test(help),
    supportsNoPromptTemplates: unknown ? true : /--no-prompt-templates\b/.test(help),
    supportsNoContextFiles: unknown ? true : /--no-context-files\b/.test(help),
    supportsNoApprove: unknown ? true : /--no-approve\b/.test(help),
    unknown,
  };
}

function addRequiredArgs(args: string[], options: BuildProviderCommandOptions): void {
  const features = optionFeatures(options);
  if (features.supportsJsonMode !== false) args.push('--mode', 'json');
  if (features.supportsNoSession !== false) args.push('--no-session');
  if (features.supportsNoExtensions !== false) args.push('--no-extensions');
  if (features.supportsNoSkills !== false) args.push('--no-skills');
  if (features.supportsNoPromptTemplates !== false) args.push('--no-prompt-templates');
  if (features.supportsNoContextFiles !== false) args.push('--no-context-files');
  if (features.supportsNoApprove !== false) args.push('--no-approve');
}

function addOptionalArgs(args: string[], options: BuildProviderCommandOptions): void {
  const features = optionFeatures(options);
  if (options.modelSpec?.model && features.supportsModel !== false) {
    args.push('--model', options.modelSpec.model);
  }
}

function failClosedUnsupportedSessionControl(options: BuildProviderCommandOptions): void {
  const hasResumeSessionId = options.resumeSessionId !== undefined;
  if (!hasResumeSessionId && !options.continueSession) return;
  const field = hasResumeSessionId ? 'options.resumeSessionId' : 'options.continueSession';
  throw contractError({
    code: 'invalid-field',
    field,
    exitCode: 2,
    message:
      'Pi CLI does not support resume/continue session control; fail closed and start a fresh run instead.',
  });
}

function collectWarnings(options: BuildProviderCommandOptions): WarningMetadata[] {
  const features = optionFeatures(options);
  const warnings: WarningMetadata[] = [];

  if (options.jsonSchema) {
    warnings.push(
      warning(
        'pi',
        'pi-jsonschema',
        'Pi CLI does not support provider-native JSON schema; appending schema instructions to the prompt.'
      )
    );
  }
  if (features.supportsJsonMode === false) {
    warnings.push(
      warning('pi', 'pi-json-mode', 'Pi CLI does not advertise --mode json; continuing anyway.')
    );
  }
  return warnings;
}

function buildCommand(context: string, options: BuildProviderCommandOptions = {}): CommandSpec {
  failClosedUnsupportedSessionControl(options);
  const finalContext = options.jsonSchema
    ? appendJsonSchemaPrompt(context, options.jsonSchema)
    : context;
  const args: string[] = [];

  addRequiredArgs(args, options);
  addOptionalArgs(args, options);
  args.push(finalContext);

  return commandSpec({
    binary: 'pi',
    args,
    env: {},
    ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
    warnings: collectWarnings(options),
  });
}

function createPiState(): ProviderParserState {
  return {
    ...createParserState('pi'),
    lastAssistantText: '',
    lastAssistantThinking: '',
  };
}

function assistantSnapshot(message: Record<string, unknown>): { text: string; thinking: string } {
  if (getString(message, 'role') !== 'assistant') return { text: '', thinking: '' };

  let text = '';
  let thinking = '';
  for (const item of getArray(message, 'content')) {
    if (!isRecord(item)) continue;
    const type = getString(item, 'type');
    if (type === 'text') {
      text += getString(item, 'text') ?? '';
    } else if (type === 'thinking') {
      thinking += getString(item, 'thinking') ?? '';
    }
  }

  return { text, thinking };
}

function snapshotDelta(previous: string, current: string): string | null {
  if (!current) return null;
  if (previous === current) return null;
  if (current.startsWith(previous)) return current.slice(previous.length) || null;
  return current;
}

function emitAssistantSnapshot(
  message: Record<string, unknown>,
  state: ProviderParserState
): readonly OutputEvent[] {
  const snapshot = assistantSnapshot(message);
  const events: OutputEvent[] = [];

  const textDelta = snapshotDelta(state.lastAssistantText ?? '', snapshot.text);
  if (textDelta) events.push({ type: 'text', text: textDelta });
  state.lastAssistantText = snapshot.text;

  const thinkingDelta = snapshotDelta(state.lastAssistantThinking ?? '', snapshot.thinking);
  if (thinkingDelta) events.push({ type: 'thinking', text: thinkingDelta });
  state.lastAssistantThinking = snapshot.thinking;

  return events;
}

function parseAssistantEvent(
  event: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent | null {
  const type = getString(event, 'type');
  if (type === 'text_delta') {
    const delta = getString(event, 'delta');
    if (!delta) return null;
    state.lastAssistantText += delta;
    return { type: 'text', text: delta };
  }
  if (type === 'thinking_delta') {
    const delta = getString(event, 'delta');
    if (!delta) return null;
    state.lastAssistantThinking += delta;
    return { type: 'thinking', text: delta };
  }
  return null;
}

function parseToolExecutionStart(
  event: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent {
  const toolId = getOptionalString(event, 'toolCallId');
  state.lastToolId = toolId;
  return {
    type: 'tool_call',
    toolName: getOptionalString(event, 'toolName'),
    toolId,
    input: event.args ?? {},
  };
}

function parseToolExecutionUpdate(
  event: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent | null {
  const toolId = getOptionalString(event, 'toolCallId') ?? state.lastToolId;
  if (toolId !== undefined) state.lastToolId = toolId;
  if (!Object.prototype.hasOwnProperty.call(event, 'partialResult')) return null;
  return {
    type: 'tool_result',
    toolId,
    content: event.partialResult,
    isError: false,
  };
}

function parseToolExecutionEnd(
  event: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent {
  const toolId = getOptionalString(event, 'toolCallId') ?? state.lastToolId;
  return {
    type: 'tool_result',
    toolId,
    content: event.result ?? '',
    isError: getBoolean(event, 'isError') ?? false,
  };
}

function parseTurnEnd(
  event: Record<string, unknown>,
  state: ProviderParserState
): readonly OutputEvent[] {
  const message = getRecord(event, 'message');
  const events: OutputEvent[] = [];
  if (message !== null) {
    events.push(...emitAssistantSnapshot(message, state));
  }

  const usage = message ? getRecord(message, 'usage') ?? {} : {};
  const stopReason = message ? getString(message, 'stopReason') : null;
  const errorMessage = message ? getString(message, 'errorMessage') : null;
  const snapshot = message ? assistantSnapshot(message) : { text: '', thinking: '' };
  const success = stopReason !== 'error' && stopReason !== 'aborted' && !errorMessage;
  events.push({
    type: 'result',
    success,
    result: success ? snapshot.text || null : null,
    error: success ? null : (errorMessage ?? stopReason ?? 'Pi turn failed'),
    inputTokens: getNumber(usage, 'input') ?? 0,
    outputTokens: getNumber(usage, 'output') ?? 0,
    cacheReadInputTokens: getNumber(usage, 'cacheRead') ?? 0,
    cacheCreationInputTokens: getNumber(usage, 'cacheWrite') ?? 0,
    cost: getRecord(usage, 'cost') ?? null,
    modelUsage: usage,
  });
  return events;
}

function parseMessageEvent(
  type: string,
  event: Record<string, unknown>,
  state: ProviderParserState
): readonly OutputEvent[] | OutputEvent | null | undefined {
  if (type === 'message_start') {
    const message = getRecord(event, 'message');
    if (message !== null && getString(message, 'role') === 'assistant') {
      state.lastAssistantText = '';
      state.lastAssistantThinking = '';
    }
    return null;
  }

  if (type === 'message_update') {
    const assistantMessageEvent = getRecord(event, 'assistantMessageEvent');
    if (assistantMessageEvent !== null) {
      const assistantEvent = parseAssistantEvent(assistantMessageEvent, state);
      if (assistantEvent !== null) return assistantEvent;
    }
    const message = getRecord(event, 'message');
    return message === null ? null : emitAssistantSnapshot(message, state);
  }

  if (type !== 'message_end') return undefined;
  const message = getRecord(event, 'message');
  return message === null ? null : emitAssistantSnapshot(message, state);
}

function parseToolEvent(
  type: string,
  event: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent | null | undefined {
  if (type === 'tool_execution_start') return parseToolExecutionStart(event, state);
  if (type === 'tool_execution_update') return parseToolExecutionUpdate(event, state);
  if (type === 'tool_execution_end') return parseToolExecutionEnd(event, state);
  return undefined;
}

function parseEvent(
  line: string,
  state: ProviderParserState
): readonly OutputEvent[] | OutputEvent | null {
  const parsed = tryParseJson(line);
  if (!isRecord(parsed)) return null;

  const type = getString(parsed, 'type');
  if (type === null || IGNORED_EVENT_TYPES.has(type)) return null;

  const messageEvent = parseMessageEvent(type, parsed, state);
  if (messageEvent !== undefined) return messageEvent;

  const toolEvent = parseToolEvent(type, parsed, state);
  if (toolEvent !== undefined) return toolEvent;

  if (type === 'turn_end') return parseTurnEnd(parsed, state);
  return null;
}

function resolveModelSpec(level: ModelLevel, overrides?: LevelOverrides): ResolvedModelSpec {
  return resolveModelSpecWithConfig({
    mapping: LEVEL_MAPPING,
    defaultLevel: 'level2',
    level,
    overrides,
    validateModelId,
  });
}

function validateModelId(modelId: string | null | undefined): string | null | undefined {
  if (modelId === undefined || modelId === null) return modelId;
  if (typeof modelId !== 'string') {
    throw new Error(`Invalid model "${unknownToMessage(modelId)}" for provider "pi".`);
  }
  return modelId;
}

function classifyError(error: unknown): ErrorClassification {
  return classifyBaseProviderError(
    error,
    [
      /\brate(?:[_ -]?limit| limited)\b/i,
      /\bquota\b/i,
      /\bresource[_ -]?exhausted\b/i,
      /\btemporar(?:y|ily)\b/i,
      /\boverloaded\b/i,
      /\bservice unavailable\b/i,
    ],
    [
      /\b(cancelled|canceled|aborted|interrupted)\b/i,
      /\brun\s*\/login\b/i,
      /\bmissing api key\b/i,
      /\bno valid authentication\b/i,
      /\bunknown option\b/i,
      /\bfailed to load\b/i,
      /\bcannot find module\b/i,
      /\bno such file or directory\b/i,
    ]
  );
}

export const piAdapter: ProviderAdapter = {
  id: 'pi',
  displayName: 'Pi',
  binary: 'pi',
  adapterVersion: '1',
  credentialEnvKeys: [],
  modelCatalog: MODEL_CATALOG,
  levelMapping: LEVEL_MAPPING,
  defaultLevel: 'level2',
  defaultMaxLevel: 'level3',
  defaultMinLevel: 'level1',
  detectCliFeatures,
  buildCommand,
  parseEvent,
  createParserState: createPiState,
  resolveModelSpec,
  validateModelId,
  classifyError,
};
