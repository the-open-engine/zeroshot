import { isRecord } from './json';
import type { RedactionMetadata } from './types';

const SECRET_KEY_PATTERN = /TOKEN|KEY|SECRET|PASSWORD/i;
const REDACTED_PREFIX = '[REDACTED:';
const SECRET_VALUE_PATTERNS: readonly RegExp[] = [
  /\b(?:sk|rk|pk)-[A-Za-z0-9][A-Za-z0-9._-]{5,}\b/g,
  /\bgithub_pat_[A-Za-z0-9_]{20,}\b/g,
  /\bgh[pousr]_[A-Za-z0-9]{20,}\b/g,
  /\bxox[baprs]?-[A-Za-z0-9-]{10,}\b/g,
  /\bAIza[A-Za-z0-9_-]{20,}\b/g,
];
const UNTRUSTED_OUTPUT_PATHS = new Set([
  'error.message',
  'evidence.message',
  'evidence.code',
  'result.evidence.message',
  'result.evidence.code',
  'result.evidence.stdout',
  'result.evidence.stderr',
]);

interface RedactionTarget {
  readonly kind: RedactionMetadata['kind'];
  readonly key: string;
  readonly source?: string;
  readonly scope: 'env-or-untrusted' | 'global' | 'untrusted';
  readonly value: string;
}

export interface RedactionResult {
  readonly value: unknown;
  readonly redactions: readonly RedactionMetadata[];
}

function metadataFor(target: RedactionTarget): RedactionMetadata {
  if (target.source === undefined) return { kind: target.kind, key: target.key };
  return { kind: target.kind, key: target.key, source: target.source };
}

function metadataKey(metadata: RedactionMetadata): string {
  return `${metadata.kind}:${metadata.key}:${metadata.source ?? ''}`;
}

function dedupeMetadata(metadata: readonly RedactionMetadata[]): readonly RedactionMetadata[] {
  const seen = new Set<string>();
  const result: RedactionMetadata[] = [];
  for (const item of metadata) {
    const key = metadataKey(item);
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(item);
  }
  return result;
}

function targetsFromEnv(env: Readonly<Record<string, string>>): readonly RedactionTarget[] {
  return Object.entries(env)
    .filter(([, value]) => value.length > 0)
    .map(([key, value]) => ({ kind: 'env', key, scope: 'env-or-untrusted', value }));
}

function shouldRedactSecretKey(key: string): boolean {
  return SECRET_KEY_PATTERN.test(key);
}

function isRedactionMetadataRecord(value: unknown): boolean {
  if (!isRecord(value)) return false;
  const keys = Object.keys(value);
  if (!keys.every((key) => key === 'kind' || key === 'key' || key === 'source')) return false;
  if (value.kind !== 'env' && value.kind !== 'secret-key' && value.kind !== 'secret-value') {
    return false;
  }
  if (typeof value.key !== 'string') return false;
  return value.source === undefined || typeof value.source === 'string';
}

function isRedactionMetadataPath(path: string): boolean {
  return /(^|\.)redactions\[\d+\]$/.test(path);
}

function shouldPreserveRedactionMetadataRecord(value: unknown, path: string): boolean {
  return isRedactionMetadataRecord(value) && isRedactionMetadataPath(path);
}

function isCredentialPresenceRecord(value: unknown): boolean {
  if (!isRecord(value)) return false;
  const keys = Object.keys(value);
  return (
    keys.length === 2 &&
    typeof value.key === 'string' &&
    typeof value.present === 'boolean' &&
    keys.every((key) => key === 'key' || key === 'present')
  );
}

function isProbeCredentialPresencePath(path: string): boolean {
  return /^result\.credentials\[\d+\]$/.test(path);
}

function shouldPreserveCredentialPresenceRecord(value: unknown, path: string): boolean {
  return isCredentialPresenceRecord(value) && isProbeCredentialPresencePath(path);
}

function collectSecretKeyTargets(value: unknown, path = ''): readonly RedactionTarget[] {
  if (Array.isArray(value)) {
    return value.flatMap((item, index) => collectSecretKeyTargets(item, `${path}[${index}]`));
  }
  if (!isRecord(value)) return [];
  if (shouldPreserveRedactionMetadataRecord(value, path)) return [];
  if (shouldPreserveCredentialPresenceRecord(value, path)) return [];

  const targets: RedactionTarget[] = [];
  for (const [key, item] of Object.entries(value)) {
    const keyPath = path ? `${path}.${key}` : key;
    if (typeof item === 'string' && item.length > 0 && shouldRedactSecretKey(key)) {
      targets.push({ kind: 'secret-key', key: keyPath, scope: 'untrusted', value: item });
    }
    targets.push(...collectSecretKeyTargets(item, keyPath));
  }
  return targets;
}

function collectSecretValueTargets(value: unknown, path = ''): readonly RedactionTarget[] {
  if (typeof value === 'string') {
    return SECRET_VALUE_PATTERNS.flatMap((pattern) =>
      collectSecretValuePatternTargets(value, path, pattern)
    );
  }
  if (Array.isArray(value)) {
    return value.flatMap((item, index) => collectSecretValueTargets(item, `${path}[${index}]`));
  }
  if (!isRecord(value)) return [];
  if (shouldPreserveRedactionMetadataRecord(value, path)) return [];
  if (shouldPreserveCredentialPresenceRecord(value, path)) return [];

  return Object.entries(value).flatMap(([key, item]) => {
    const keyPath = path ? `${path}.${key}` : key;
    return collectSecretValueTargets(item, keyPath);
  });
}

function collectSecretValuePatternTargets(
  value: string,
  path: string,
  pattern: RegExp
): readonly RedactionTarget[] {
  const targets: RedactionTarget[] = [];
  pattern.lastIndex = 0;
  let match = pattern.exec(value);
  while (match !== null) {
    const secret = match[0];
    if (secret) {
      targets.push({
        kind: 'secret-value',
        key: path || 'value',
        scope: 'global',
        value: secret,
      });
    }
    match = pattern.exec(value);
  }
  return targets;
}

function applyTargets(input: string, targets: readonly RedactionTarget[]): string {
  let output = input;
  for (const target of targets) {
    if (!target.value) continue;
    output = output.split(target.value).join(`${REDACTED_PREFIX}${target.key}]`);
  }
  return output;
}

function envTargetPathKey(target: RedactionTarget): string {
  return target.key.split(':', 1)[0] ?? target.key;
}

function isEnvValuePath(path: string, target: RedactionTarget): boolean {
  const key = envTargetPathKey(target);
  return path === `env.${key}` || path.endsWith(`.env.${key}`);
}

function isUntrustedOutputPath(path: string): boolean {
  if (path.length === 0 || path === 'value') return true;
  return (
    UNTRUSTED_OUTPUT_PATHS.has(path) ||
    path.startsWith('result.events') ||
    path.startsWith('result.diagnostics')
  );
}

function shouldApplyTargetAtPath(target: RedactionTarget, path: string): boolean {
  if (target.scope === 'global') return true;
  if (target.scope === 'untrusted') return isUntrustedOutputPath(path);
  return isEnvValuePath(path, target) || isUntrustedOutputPath(path);
}

function applyTargetsAtPath(
  input: string,
  targets: readonly RedactionTarget[],
  path: string
): string {
  return applyTargets(
    input,
    targets.filter((target) => shouldApplyTargetAtPath(target, path))
  );
}

function redactValue(value: unknown, targets: readonly RedactionTarget[], path = ''): unknown {
  if (typeof value === 'string') return applyTargetsAtPath(value, targets, path);
  if (Array.isArray(value)) {
    return value.map((item, index) => redactValue(item, targets, `${path}[${index}]`));
  }
  if (!isRecord(value)) return value;
  if (shouldPreserveRecord(value, path)) return value;
  return redactRecord(value, targets, path);
}

function shouldPreserveRecord(value: Record<string, unknown>, path: string): boolean {
  return (
    shouldPreserveRedactionMetadataRecord(value, path) ||
    shouldPreserveCredentialPresenceRecord(value, path)
  );
}

function redactRecord(
  value: Record<string, unknown>,
  targets: readonly RedactionTarget[],
  path: string
): Record<string, unknown> {
  const redacted: Record<string, unknown> = {};
  for (const [key, item] of Object.entries(value)) {
    const keyPath = path ? `${path}.${key}` : key;
    redacted[key] = redactRecordItem(key, item, targets, keyPath);
  }
  return redacted;
}

function redactRecordItem(
  key: string,
  item: unknown,
  targets: readonly RedactionTarget[],
  path: string
): unknown {
  if (typeof item === 'string' && item.length > 0 && shouldRedactSecretKey(key)) {
    return `${REDACTED_PREFIX}${key}]`;
  }
  return redactValue(item, targets, path);
}

export function redactString(
  value: string,
  env: Readonly<Record<string, string>> = {}
): RedactionResult {
  const targets = targetsFromEnv(env);
  return {
    value: applyTargetsAtPath(value, targets, 'value'),
    redactions: dedupeMetadata(targets.map(metadataFor)),
  };
}

export function redactObject(
  value: unknown,
  env: Readonly<Record<string, string>> = {}
): RedactionResult {
  const targets = [
    ...targetsFromEnv(env),
    ...collectSecretKeyTargets(value),
    ...collectSecretValueTargets(value),
  ];
  return {
    value: redactValue(value, targets),
    redactions: dedupeMetadata(targets.map(metadataFor)),
  };
}

export function mergeRedactions(
  ...metadataGroups: readonly (readonly RedactionMetadata[] | undefined)[]
): readonly RedactionMetadata[] {
  return dedupeMetadata(metadataGroups.flatMap((metadata) => metadata ?? []));
}
