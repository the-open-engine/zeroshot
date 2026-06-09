import { getProviderAdapter } from './adapters';
import { findUnsafeProviderEnvKey } from './env-safety';
import { isRecord } from './json';
import { mergeRedactions } from './redaction';
import { contractError } from './contract-errors';
import type { CommandSpec, ProviderAdapter, RedactionMetadata } from './types';

export function stringRecord(value: unknown, field: string): Readonly<Record<string, string>> {
  if (value === undefined) return {};
  if (!isRecord(value)) {
    throw contractError({
      code: 'invalid-field',
      message: `${field} must be an object with string values.`,
      exitCode: 2,
      field,
    });
  }

  const result: Record<string, string> = {};
  for (const [key, item] of Object.entries(value)) {
    if (typeof item !== 'string') {
      throw contractError({
        code: 'invalid-field',
        message: `${field}.${key} must be a string.`,
        exitCode: 2,
        field: `${field}.${key}`,
      });
    }
    result[key] = item;
  }
  assertNoUnsafeProviderEnv(result, field);
  return result;
}

function assertNoUnsafeProviderEnv(env: Readonly<Record<string, string>>, field: string): void {
  const key = findUnsafeProviderEnvKey(env);
  if (key === null) return;
  throw contractError({
    code: 'forbidden-field',
    message: `${field}.${key} is not accepted by provider executable requests; provider adapters own executable resolution and process-control environment.`,
    exitCode: 2,
    field: `${field}.${key}`,
  });
}

export function envRedactions(env: Readonly<Record<string, string>>): readonly RedactionMetadata[] {
  return Object.keys(env).map((key) => ({ kind: 'env', key }));
}

export function commandRedactions(commandSpec: CommandSpec): readonly RedactionMetadata[] {
  return mergeRedactions(commandSpec.redactions, envRedactions(commandSpec.env));
}

export function providerCredentialEnv(provider: string | null): Readonly<Record<string, string>> {
  if (provider === null) return {};
  let adapter: ProviderAdapter;
  try {
    adapter = getProviderAdapter(provider);
  } catch {
    return {};
  }
  const result: Record<string, string> = {};
  for (const key of adapter.credentialEnvKeys) {
    const value = process.env[key];
    if (typeof value === 'string' && value.length > 0) result[key] = value;
  }
  return result;
}

export function mergeEnvForRedaction(
  ...inputs: readonly {
    readonly source: string;
    readonly env: Readonly<Record<string, string>>;
  }[]
): Readonly<Record<string, string>> {
  const result: Record<string, string> = {};
  for (const input of inputs) {
    for (const [key, value] of Object.entries(input.env)) {
      if (value.length === 0) continue;
      if (result[key] === undefined || result[key] === value) {
        result[key] = value;
        continue;
      }
      result[`${key}:${input.source}`] = value;
    }
  }
  return result;
}

export function collectCommandSpecEnv(value: unknown): Readonly<Record<string, string>> {
  const result: Record<string, string> = {};
  collectCommandSpecEnvInto(value, result);
  return result;
}

function collectCommandSpecEnvInto(value: unknown, result: Record<string, string>): void {
  if (Array.isArray(value)) {
    for (const item of value) collectCommandSpecEnvInto(item, result);
    return;
  }
  if (!isRecord(value)) return;

  const commandSpec = value.commandSpec;
  if (isRecord(commandSpec) && isRecord(commandSpec.env)) {
    for (const [key, item] of Object.entries(commandSpec.env)) {
      if (typeof item === 'string' && item.length > 0) result[key] = item;
    }
  }

  for (const item of Object.values(value)) collectCommandSpecEnvInto(item, result);
}
