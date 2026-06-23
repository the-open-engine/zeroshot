import { appendJsonSchemaPrompt } from '../schema';
import { getArray, getOrStringFromKeys, getString, isRecord, tryParseJson } from '../json';
import {
  type BuildProviderCommandOptions,
  type CommandSpec,
  type ErrorClassification,
  type KiroCliFeatures,
  type LevelModelSpec,
  type LevelOverrides,
  type ModelCatalogEntry,
  type ModelLevel,
  type OutputEvent,
  type ProviderAdapter,
  type ProviderParseResult,
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

// kiro-cli is AWS-backed and serves Claude-family models via Bedrock. A non-empty
// catalog keeps model validation fail-fast (unknown ids throw) while accepting real
// model names. Levels resolve to null so normal runs defer to kiro's configured
// default (kiro-cli settings chat.defaultModel); the catalog only gates explicit
// per-agent model overrides.
const MODEL_CATALOG: Readonly<Record<string, ModelCatalogEntry>> = {
  'claude-haiku-4-5': { rank: 1 },
  'claude-sonnet-4-5': { rank: 2 },
  'claude-opus-4-1': { rank: 3 },
};

const LEVEL_MAPPING: Readonly<Record<ModelLevel, LevelModelSpec>> = {
  level1: { rank: 1, model: null },
  level2: { rank: 2, model: null },
  level3: { rank: 3, model: null },
};

function detectCliFeatures(helpText?: string | null): KiroCliFeatures {
  const help = helpText ?? '';
  const unknown = !help;
  return {
    provider: 'kiro',
    supportsJson: unknown ? true : /--output-format\b/.test(help),
    supportsNoInteractive: unknown ? true : /--no-interactive\b/.test(help),
    supportsTrustAllTools: unknown ? true : /--trust-all-tools\b/.test(help),
    // Headless kiro documents no per-invocation model flag, so default off and only
    // emit --model when an installed CLI advertises it.
    supportsModel: unknown ? false : /--model\b/.test(help),
    supportsCwd: unknown ? false : /--cwd\b/.test(help),
    unknown,
  };
}

function addKiroOptionalArgs(args: string[], options: BuildProviderCommandOptions): void {
  const features = optionFeatures(options);
  if (features.supportsNoInteractive !== false) {
    args.push('--no-interactive');
  }

  if (
    (options.outputFormat === 'stream-json' || options.outputFormat === 'json') &&
    features.supportsJson
  ) {
    args.push('--output-format', 'json');
  }

  if (options.autoApprove && features.supportsTrustAllTools) {
    args.push('--trust-all-tools');
  }

  if (options.modelSpec?.model && features.supportsModel) {
    args.push('--model', options.modelSpec.model);
  }

  if (options.cwd && features.supportsCwd) {
    args.push('--cwd', options.cwd);
  }
}

function collectKiroWarnings(options: BuildProviderCommandOptions): WarningMetadata[] {
  const features = optionFeatures(options);
  const warnings: WarningMetadata[] = unsupportedSessionControlWarnings('kiro', options);
  if (options.modelSpec?.model && features.supportsModel === false) {
    warnings.push(
      warning(
        'kiro',
        'kiro-model',
        'kiro-cli has no per-invocation model flag in headless mode; ignoring model selection.'
      )
    );
  }
  if (options.autoApprove && features.supportsTrustAllTools === false) {
    warnings.push(
      warning(
        'kiro',
        'kiro-auto-approve',
        'kiro-cli does not support --trust-all-tools; continuing without auto-approve.'
      )
    );
  }
  return warnings;
}

function buildCommand(context: string, options: BuildProviderCommandOptions = {}): CommandSpec {
  const finalContext = options.jsonSchema
    ? appendJsonSchemaPrompt(context, options.jsonSchema)
    : context;
  const args: string[] = ['chat'];
  addKiroOptionalArgs(args, options);
  args.push(finalContext);

  return commandSpec({
    binary: 'kiro-cli',
    // kiro-cli renders colored/pretty output even when piped; NO_COLOR keeps
    // stdout (and logs) clean so the agent's JSON answer is easy to recover.
    env: { NO_COLOR: '1' },
    args,
    ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
    warnings: collectKiroWarnings(options),
  });
}

function parseToolCalls(obj: Record<string, unknown>, state: ProviderParserState): OutputEvent[] {
  const events: OutputEvent[] = [];
  for (const call of getArray(obj, 'tool_calls')) {
    if (!isRecord(call)) continue;
    const toolId = getOrStringFromKeys(call, ['tool_call_id', 'id', 'tool_id']);
    if (toolId) state.lastToolId = toolId;
    events.push({
      type: 'tool_call',
      toolName: getOrStringFromKeys(call, ['name', 'tool_name']),
      toolId,
      input: call.arguments ?? call.input ?? call.parameters ?? {},
    });
  }
  return events;
}

// kiro-cli (--output-format json) emits a single final JSON object on stdout:
// { response, tool_calls, stop_reason }. One line yields multiple OutputEvents.
function parseEvent(line: string, state: ProviderParserState): ProviderParseResult {
  const obj = tryParseJson(line);
  if (!isRecord(obj)) return null;

  const response = getString(obj, 'response');
  const stop = getString(obj, 'stop_reason');
  const hasToolCalls = Array.isArray(obj.tool_calls);
  if (response === null && stop === null && !hasToolCalls) return null;

  const events: OutputEvent[] = parseToolCalls(obj, state);

  if (response) events.push({ type: 'text', text: response });

  events.push({
    type: 'result',
    success: stop !== 'error',
    result: response ?? '',
    error: stop === 'error' ? (getString(obj, 'error') ?? 'kiro-cli returned an error') : null,
  });

  return events;
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
  return validateModelIdFromCatalog('kiro', MODEL_CATALOG, modelId);
}

function classifyError(error: unknown): ErrorClassification {
  return classifyBaseProviderError(
    error,
    [
      /\bThrottlingException\b/i,
      /\bTooManyRequestsException\b/i,
      /\bServiceUnavailable(Exception)?\b/i,
      /\bServiceQuotaExceeded/i,
      /\bInternalServerException\b/i,
      /throttl/i,
      /rate.?limit/i,
    ],
    [
      /\bAccessDenied(Exception)?\b/i,
      /\bUnrecognizedClientException\b/i,
      /\bValidationException\b/i,
      /\bResourceNotFoundException\b/i,
      /\bExpiredTokenException\b/i,
      /\bUnauthorizedException\b/i,
    ]
  );
}

export const kiroAdapter: ProviderAdapter = {
  id: 'kiro',
  displayName: 'Kiro',
  binary: 'kiro-cli',
  adapterVersion: '1',
  credentialEnvKeys: ['KIRO_API_KEY'],
  modelCatalog: MODEL_CATALOG,
  levelMapping: LEVEL_MAPPING,
  defaultLevel: 'level2',
  defaultMaxLevel: 'level3',
  defaultMinLevel: 'level1',
  detectCliFeatures,
  buildCommand,
  parseEvent,
  createParserState: () => createParserState('kiro'),
  resolveModelSpec,
  validateModelId,
  classifyError,
};
