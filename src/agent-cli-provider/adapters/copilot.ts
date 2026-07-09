import { appendJsonSchemaPrompt } from '../schema';
import { unknownToMessage } from '../json';
import {
  type BuildProviderCommandOptions,
  type CommandSpec,
  type CopilotCliFeatures,
  type ErrorClassification,
  type LevelModelSpec,
  type LevelOverrides,
  type ModelCatalogEntry,
  type ModelLevel,
  type ProviderAdapter,
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
  warning,
} from './common';
import { parseCopilotEvent } from './copilot-parser';
import type { ProviderParserState } from '../types';

// Empty catalog: Copilot's models are plan-dependent, so modelLevel is a no-op (uses Copilot's
// default). Pin a model via the `model` field or COPILOT_MODEL.
const MODEL_CATALOG: Readonly<Record<string, ModelCatalogEntry>> = {};

const LEVEL_MAPPING: Readonly<Record<ModelLevel, LevelModelSpec>> = {
  level1: { rank: 1, model: null },
  level2: { rank: 2, model: null },
  level3: { rank: 3, model: null },
};

function createCopilotState(): ProviderParserState {
  return {
    ...createParserState('copilot'),
    lastAssistantText: '',
    messagePhaseById: new Map(),
    assistantTextByMessageId: new Map(),
    usage: { outputTokens: 0 },
  };
}

function supports(help: string, pattern: RegExp): boolean {
  return help ? pattern.test(help) : true;
}

function detectCliFeatures(helpText?: string | null): CopilotCliFeatures {
  const help = helpText ?? '';
  const unknown = !help;
  return {
    provider: 'copilot',
    supportsJsonOutput: supports(help, /--output-format\b/),
    supportsModel: supports(help, /--model\b/),
    supportsAllowAll: supports(help, /--allow-all\b/),
    supportsNoAskUser: supports(help, /--no-ask-user\b/),
    supportsAddDir: supports(help, /--add-dir\b/),
    unknown,
  };
}

function addOutputArgs(args: string[], options: BuildProviderCommandOptions): void {
  const features = optionFeatures(options);
  if (
    (options.outputFormat === 'json' || options.outputFormat === 'stream-json') &&
    features.supportsJsonOutput !== false
  ) {
    args.push('--output-format', 'json');
  }
}

function addModelArgs(args: string[], options: BuildProviderCommandOptions): void {
  const features = optionFeatures(options);
  if (options.modelSpec?.model && features.supportsModel !== false) {
    args.push('--model', options.modelSpec.model);
  }
}

function addAddDirArgs(args: string[], options: BuildProviderCommandOptions): void {
  const features = optionFeatures(options);
  if (options.cwd && features.supportsAddDir !== false) {
    args.push('--add-dir', options.cwd);
  }
}

function addAutoApproveArgs(args: string[], options: BuildProviderCommandOptions): void {
  const features = optionFeatures(options);
  if (!options.autoApprove) return;
  if (features.supportsAllowAll !== false) args.push('--allow-all');
  if (features.supportsNoAskUser !== false) args.push('--no-ask-user');
}

function collectWarnings(options: BuildProviderCommandOptions): WarningMetadata[] {
  const features = optionFeatures(options);
  const warnings: WarningMetadata[] = unsupportedSessionControlWarnings('copilot', options);

  if (options.jsonSchema) {
    warnings.push(
      warning(
        'copilot',
        'copilot-jsonschema',
        'Copilot CLI does not support provider-native JSON schema; appending schema instructions to the prompt.'
      )
    );
  }
  if (options.autoApprove && features.supportsAllowAll === false) {
    warnings.push(
      warning(
        'copilot',
        'copilot-auto-approve',
        'Copilot CLI does not advertise --allow-all; continuing without the tool auto-approve flag.'
      )
    );
  }
  return warnings;
}

function buildCommand(context: string, options: BuildProviderCommandOptions = {}): CommandSpec {
  const finalContext = options.jsonSchema
    ? appendJsonSchemaPrompt(context, options.jsonSchema)
    : context;
  const args: string[] = [];

  addOutputArgs(args, options);
  addModelArgs(args, options);
  addAddDirArgs(args, options);
  addAutoApproveArgs(args, options);
  args.push('-p', finalContext);

  return commandSpec({
    binary: 'copilot',
    args,
    env: {},
    ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
    warnings: collectWarnings(options),
  });
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
    throw new Error(`Invalid model "${unknownToMessage(modelId)}" for provider "copilot".`);
  }
  return modelId;
}

function classifyError(error: unknown): ErrorClassification {
  return classifyBaseProviderError(
    error,
    [
      /\brate(?:[_ -]?limit| limited)\b/i,
      /\b429\b/,
      /\boverloaded\b/i,
      /\bservice unavailable\b/i,
      /\b503\b/,
      /\btemporar(?:y|ily)\b/i,
      /\btimeout\b/i,
    ],
    [
      /\bunauthorized\b/i,
      /\bforbidden\b/i,
      /\bbad credentials\b/i,
      /\bauthentication\b/i,
      /\binvalid token\b/i,
      /\b(GH_TOKEN|GITHUB_TOKEN|COPILOT_GITHUB_TOKEN)\b/,
      /\bquota\b/i,
      /\b(cancelled|canceled|aborted|interrupted)\b/i,
      /\bunknown option\b/i,
    ]
  );
}

export const copilotAdapter: ProviderAdapter = {
  id: 'copilot',
  displayName: 'Copilot',
  binary: 'copilot',
  adapterVersion: '1',
  credentialEnvKeys: ['COPILOT_GITHUB_TOKEN', 'GH_TOKEN', 'GITHUB_TOKEN'],
  modelCatalog: MODEL_CATALOG,
  levelMapping: LEVEL_MAPPING,
  defaultLevel: 'level2',
  defaultMaxLevel: 'level3',
  defaultMinLevel: 'level1',
  detectCliFeatures,
  buildCommand,
  parseEvent: parseCopilotEvent,
  createParserState: createCopilotState,
  resolveModelSpec,
  validateModelId,
  classifyError,
};
