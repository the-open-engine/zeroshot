import { isRecord } from './json';
import { invalidField, contractError } from './contract-errors';
import { stringRecord } from './contract-env';
import type {
  BuildProviderCommandOptions,
  CliFeatureOverrides,
  ModelLevel,
  ModelSpec,
  OutputFormat,
  ReasoningEffort,
} from './types';

const OUTPUT_FORMATS: readonly OutputFormat[] = ['text', 'json', 'stream-json'];
const MODEL_LEVELS: readonly ModelLevel[] = ['level1', 'level2', 'level3'];
const REASONING_EFFORTS: readonly ReasoningEffort[] = ['low', 'medium', 'high', 'xhigh'];
const CLI_FEATURE_FIELDS = [
  'supportsOutputFormat',
  'supportsStreamJson',
  'supportsJsonSchema',
  'supportsAutoApprove',
  'supportsIncludePartials',
  'supportsVerbose',
  'supportsModel',
  'supportsJson',
  'supportsOutputSchema',
  'supportsCwd',
  'supportsConfigOverride',
  'supportsSkipGitRepoCheck',
  'supportsVariant',
  'unknown',
] as const;

type CliFeatureField = (typeof CLI_FEATURE_FIELDS)[number];

export function requestOptions(value: unknown): BuildProviderCommandOptions {
  if (value === undefined) return {};
  if (!isRecord(value)) {
    throw contractError({
      code: 'invalid-field',
      message: 'options must be an object.',
      exitCode: 2,
      field: 'options',
    });
  }
  rejectProviderCommandOverrides(value);
  return normalizeBuildOptions(value);
}

function rejectProviderCommandOverrides(value: Record<string, unknown>): void {
  for (const field of ['command', 'commandArgs']) {
    if (!Object.prototype.hasOwnProperty.call(value, field)) continue;
    throw contractError({
      code: 'forbidden-field',
      message: `options.${field} is not accepted by provider executable requests; provider adapters own executable binaries and arguments.`,
      exitCode: 2,
      field: `options.${field}`,
    });
  }
}

function optionalStringValue(value: unknown, field: string): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value === 'string') return value;
  invalidField(field, `${field} must be a string.`);
}

function optionalNullableStringValue(value: unknown, field: string): string | null | undefined {
  if (value === undefined) return undefined;
  if (value === null) return null;
  if (typeof value === 'string') return value;
  invalidField(field, `${field} must be a string or null.`);
}

function optionalBooleanValue(value: unknown, field: string): boolean | undefined {
  if (value === undefined) return undefined;
  if (typeof value === 'boolean') return value;
  invalidField(field, `${field} must be a boolean.`);
}

function optionalEnumValue<T extends string>(
  value: unknown,
  field: string,
  allowed: readonly T[]
): T | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== 'string') {
    invalidField(field, `${field} must be a string.`);
  }
  for (const item of allowed) {
    if (value === item) return item;
  }
  invalidField(field, `${field} must be one of: ${allowed.join(', ')}.`);
}

function optionalModelSpec(value: unknown): ModelSpec | undefined {
  if (value === undefined) return undefined;
  if (!isRecord(value)) {
    invalidField('options.modelSpec', 'options.modelSpec must be an object.');
  }

  const level = optionalEnumValue(value.level, 'options.modelSpec.level', MODEL_LEVELS);
  const model = optionalNullableStringValue(value.model, 'options.modelSpec.model');
  const reasoningEffort = optionalEnumValue(
    value.reasoningEffort,
    'options.modelSpec.reasoningEffort',
    REASONING_EFFORTS
  );

  return {
    ...(level === undefined ? {} : { level }),
    ...(model === undefined ? {} : { model }),
    ...(reasoningEffort === undefined ? {} : { reasoningEffort }),
  };
}

function optionalCliFeatures(value: unknown): CliFeatureOverrides | undefined {
  if (value === undefined) return undefined;
  if (!isRecord(value)) {
    invalidField('options.cliFeatures', 'options.cliFeatures must be an object.');
  }

  const result: Partial<Record<CliFeatureField, boolean>> = {};
  for (const field of CLI_FEATURE_FIELDS) {
    const item = optionalBooleanValue(value[field], `options.cliFeatures.${field}`);
    if (item !== undefined) result[field] = item;
  }
  return result;
}

function normalizeBuildOptions(value: Record<string, unknown>): BuildProviderCommandOptions {
  const result: Record<string, unknown> = {};
  addDefined(result, 'modelSpec', optionalModelSpec(value.modelSpec));
  addDefined(
    result,
    'outputFormat',
    optionalEnumValue(value.outputFormat, 'options.outputFormat', OUTPUT_FORMATS)
  );
  addPresent(result, value, 'jsonSchema');
  addDefined(result, 'cwd', optionalStringValue(value.cwd, 'options.cwd'));
  addDefined(result, 'autoApprove', optionalBooleanValue(value.autoApprove, 'options.autoApprove'));
  addDefined(
    result,
    'resumeSessionId',
    optionalStringValue(value.resumeSessionId, 'options.resumeSessionId')
  );
  addDefined(
    result,
    'continueSession',
    optionalBooleanValue(value.continueSession, 'options.continueSession')
  );
  addDefined(result, 'cliFeatures', optionalCliFeatures(value.cliFeatures));
  addDefined(
    result,
    'strictSchema',
    optionalBooleanValue(value.strictSchema, 'options.strictSchema')
  );
  if (Object.prototype.hasOwnProperty.call(value, 'authEnv')) {
    result.authEnv = stringRecord(value.authEnv, 'options.authEnv');
  }
  return result as BuildProviderCommandOptions;
}

function addDefined(result: Record<string, unknown>, key: string, value: unknown): void {
  if (value !== undefined) result[key] = value;
}

function addPresent(
  result: Record<string, unknown>,
  input: Record<string, unknown>,
  key: string
): void {
  if (Object.prototype.hasOwnProperty.call(input, key)) result[key] = input[key];
}
