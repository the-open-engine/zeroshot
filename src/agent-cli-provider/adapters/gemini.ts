import { appendJsonSchemaPrompt } from '../schema';
import {
  getOrStringFromKeys,
  getOrStringFromKeysWithFallback,
  getString,
  isRecord,
  tryParseJson,
} from '../json';
import {
  type BuildProviderCommandOptions,
  type CommandSpec,
  type ErrorClassification,
  type GeminiCliFeatures,
  type LevelModelSpec,
  type LevelOverrides,
  type ModelCatalogEntry,
  type ModelLevel,
  type OutputEvent,
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
  unsupportedSessionControlWarnings,
  validateModelIdFromCatalog,
  warning,
} from './common';

const MODEL_CATALOG: Readonly<Record<string, ModelCatalogEntry>> = {
  'gemini-2.5-pro': { rank: 3 },
  'gemini-2.0-flash': { rank: 1 },
};

const LEVEL_MAPPING: Readonly<Record<ModelLevel, LevelModelSpec>> = {
  level1: { rank: 1, model: null },
  level2: { rank: 2, model: null },
  level3: { rank: 3, model: null },
};

function detectCliFeatures(helpText?: string | null): GeminiCliFeatures {
  const help = helpText ?? '';
  const unknown = !help;
  return {
    provider: 'gemini',
    supportsStreamJson: unknown ? true : /--output-format\b/.test(help),
    supportsAutoApprove: unknown ? true : /--yolo\b/.test(help),
    supportsCwd: unknown ? true : /--cwd\b/.test(help),
    supportsModel: unknown ? true : /\s-m\b/.test(help) || /--model\b/.test(help),
    unknown,
  };
}

function addGeminiOptionalArgs(args: string[], options: BuildProviderCommandOptions): void {
  const features = optionFeatures(options);
  if (
    (options.outputFormat === 'stream-json' || options.outputFormat === 'json') &&
    features.supportsStreamJson
  ) {
    args.push('--output-format', 'stream-json');
  }

  if (options.modelSpec?.model) {
    args.push('-m', options.modelSpec.model);
  }

  if (options.cwd && features.supportsCwd) {
    args.push('--cwd', options.cwd);
  }

  if (options.autoApprove && features.supportsAutoApprove) {
    args.push('--yolo');
  }
}

function collectGeminiWarnings(options: BuildProviderCommandOptions): WarningMetadata[] {
  const features = optionFeatures(options);
  const warnings: WarningMetadata[] = unsupportedSessionControlWarnings('gemini', options);
  if (options.autoApprove && features.supportsAutoApprove === false) {
    warnings.push(
      warning(
        'gemini',
        'gemini-auto-approve',
        'Gemini CLI does not support --yolo; continuing without auto-approve.'
      )
    );
  }
  return warnings;
}

function buildCommand(context: string, options: BuildProviderCommandOptions = {}): CommandSpec {
  const finalContext = options.jsonSchema
    ? appendJsonSchemaPrompt(context, options.jsonSchema)
    : context;
  const args: string[] = ['-p', finalContext];
  addGeminiOptionalArgs(args, options);

  return commandSpec({
    binary: 'gemini',
    args,
    env: { GEMINI_CLI_TRUST_WORKSPACE: 'true' },
    ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
    warnings: collectGeminiWarnings(options),
  });
}

function normalizeMessageContent(content: unknown): string {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    return content
      .map((item) => {
        if (typeof item === 'string') return item;
        if (isRecord(item)) return getString(item, 'text') ?? '';
        return '';
      })
      .join('');
  }
  if (isRecord(content)) return getString(content, 'text') ?? '';
  return '';
}

function parseMessageEvent(event: Record<string, unknown>): OutputEvent | null {
  if (getString(event, 'role') !== 'assistant') return null;
  const text = normalizeMessageContent(event.content);
  return text ? { type: 'text', text } : null;
}

function parseToolUseEvent(
  event: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent {
  const toolId = getOrStringFromKeysWithFallback(
    event,
    ['tool_call_id', 'tool_id', 'id'],
    state.lastToolId
  );
  state.lastToolId = toolId;
  return {
    type: 'tool_call',
    toolName: getOrStringFromKeys(event, ['tool_name', 'name']),
    toolId,
    input: event.parameters ?? event.input ?? {},
  };
}

function parseToolResultEvent(
  event: Record<string, unknown>,
  state: ProviderParserState
): OutputEvent {
  const toolId = getOrStringFromKeysWithFallback(
    event,
    ['tool_call_id', 'tool_id', 'id'],
    state.lastToolId
  );
  return {
    type: 'tool_result',
    toolId,
    content: event.output ?? event.content ?? '',
    isError: event.success === false,
  };
}

function parseResultEvent(event: Record<string, unknown>): OutputEvent {
  return {
    type: 'result',
    success: event.success !== false,
    result: event.result || '',
    error: event.success === false ? (getString(event, 'error') ?? 'Result failed') : null,
  };
}

function parseEvent(line: string, state: ProviderParserState): OutputEvent | null {
  const event = tryParseJson(line);
  if (!isRecord(event)) return null;

  switch (getString(event, 'type')) {
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
      return null;
  }
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
  return validateModelIdFromCatalog('gemini', MODEL_CATALOG, modelId);
}

function classifyError(error: unknown): ErrorClassification {
  return classifyBaseProviderError(
    error,
    [
      /\bRESOURCE_EXHAUSTED\b/i,
      /\bUNAVAILABLE\b/i,
      /\bDEADLINE_EXCEEDED\b/i,
      /No capacity available/i,
      /quota.?exceeded/i,
    ],
    [
      /\bINVALID_ARGUMENT\b/i,
      /\bPERMISSION_DENIED\b/i,
      /\bNOT_FOUND\b/i,
      /\bIneligibleTierError\b/i,
      /\bUNSUPPORTED_CLIENT\b/i,
      /\bno longer supported\b/i,
    ]
  );
}

export const geminiAdapter: ProviderAdapter = {
  id: 'gemini',
  displayName: 'Gemini',
  binary: 'gemini',
  adapterVersion: '1',
  credentialEnvKeys: ['GEMINI_API_KEY', 'GOOGLE_API_KEY'],
  modelCatalog: MODEL_CATALOG,
  levelMapping: LEVEL_MAPPING,
  defaultLevel: 'level2',
  defaultMaxLevel: 'level3',
  defaultMinLevel: 'level1',
  detectCliFeatures,
  buildCommand,
  parseEvent,
  createParserState: () => createParserState('gemini'),
  resolveModelSpec,
  validateModelId,
  classifyError,
};
