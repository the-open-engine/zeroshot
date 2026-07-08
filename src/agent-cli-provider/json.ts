export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function parseJson(value: string): unknown {
  const parsed: unknown = JSON.parse(value);
  return parsed;
}

export function tryParseJson(value: string): unknown | null {
  try {
    return parseJson(value);
  } catch {
    return null;
  }
}

export function getRecord(
  record: Record<string, unknown>,
  key: string
): Record<string, unknown> | null {
  const value = record[key];
  return isRecord(value) ? value : null;
}

export function getArray(record: Record<string, unknown>, key: string): readonly unknown[] {
  const value = record[key];
  return Array.isArray(value) ? value : [];
}

export function getString(record: Record<string, unknown>, key: string): string | null {
  const value = record[key];
  return typeof value === 'string' ? value : null;
}

export function getOptionalString(
  record: Record<string, unknown>,
  key: string
): string | null | undefined {
  const value = record[key];
  if (typeof value === 'string') return value;
  if (value === null) return null;
  return undefined;
}

export function getStringFromKeys(
  record: Record<string, unknown>,
  keys: readonly string[]
): string | null {
  for (const key of keys) {
    const value = getString(record, key);
    if (value !== null) return value;
  }
  return null;
}

export function getOrStringFromKeys(
  record: Record<string, unknown>,
  keys: readonly string[]
): string | null | undefined {
  for (const key of keys) {
    const value = getOptionalString(record, key);
    if (value) return value;
  }

  const lastKey = keys[keys.length - 1];
  return lastKey === undefined ? undefined : getOptionalString(record, lastKey);
}

export function getOrStringFromKeysWithFallback(
  record: Record<string, unknown>,
  keys: readonly string[],
  fallback: string | null | undefined
): string | null | undefined {
  for (const key of keys) {
    const value = getOptionalString(record, key);
    if (value) return value;
  }

  return fallback;
}

export function getNumber(record: Record<string, unknown>, key: string): number | null {
  const value = record[key];
  return typeof value === 'number' ? value : null;
}

export function getBoolean(record: Record<string, unknown>, key: string): boolean | null {
  const value = record[key];
  return typeof value === 'boolean' ? value : null;
}

export function stringifyJson(value: unknown, space?: number): string {
  const serialized = JSON.stringify(value, null, space);
  if (typeof serialized !== 'string') {
    throw new Error('JSON schema must be serializable.');
  }
  return serialized;
}

export function stringifyContent(value: unknown): string {
  if (typeof value === 'string') return value;
  return stringifyJson(value);
}

export function unknownToMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  if (isRecord(error)) {
    const message = error.message;
    if (typeof message === 'string' && message) return message;
  }
  return String(error);
}
