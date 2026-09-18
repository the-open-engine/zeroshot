/** Display helpers only: these values never become graph inputs or executable configuration. */
export const RECORDED_PAGE_SIZE = 10;
export const RECORDED_TEXT_PAGE_SIZE = 4000;
const MAX_DECODE_BYTES = 64 * 1024;
const MAX_PATH_SEGMENTS = 32;
export const UNREADABLE_VALUE = Symbol('unreadable recorded property');

export function decodeRecordedValue(value: unknown): unknown {
  if (typeof value !== 'string' || value.length > MAX_DECODE_BYTES) return value;
  const text = value.trim();
  if (
    !(text.startsWith('{') && text.endsWith('}')) &&
    !(text.startsWith('[') && text.endsWith(']'))
  )
    return value;
  try {
    return JSON.parse(text, (_key, item: unknown) => {
      if (
        typeof item === 'number' &&
        (!Number.isFinite(item) || (Number.isInteger(item) && !Number.isSafeInteger(item)))
      )
        throw new Error('Numeric value cannot be represented exactly.');
      return item;
    });
  } catch {
    return value;
  }
}

export function isRecordedObject(value: unknown): value is Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

/** Inspect own data properties without invoking arbitrary getters. */
export function recordedProperty(value: object, key: string): unknown {
  const descriptor = Object.getOwnPropertyDescriptor(value, key);
  return descriptor && Object.hasOwn(descriptor, 'value') ? descriptor.value : UNREADABLE_VALUE;
}

/** Collapse wrapper records while retaining the exact path and cycle boundary. */
export function recordedFieldPath(key: string, source: unknown, ancestors: readonly object[] = []) {
  const path = [key];
  const lineage = [...ancestors];
  let value = decodeRecordedValue(source);
  while (path.length < MAX_PATH_SEGMENTS && isRecordedObject(value) && !lineage.includes(value)) {
    const keys = Object.keys(value);
    if (keys.length !== 1) break;
    lineage.push(value);
    path.push(keys[0]);
    value = decodeRecordedValue(recordedProperty(value, keys[0]));
  }
  return { path, value, ancestors: lineage };
}

/** Keep UTF-16 pairs together when paging text; concatenating pages reproduces the input. */
export function recordedTextRange(value: string, page: number) {
  const pages = Math.max(1, Math.ceil(value.length / RECORDED_TEXT_PAGE_SIZE));
  const current = Math.max(0, Math.min(page, pages - 1));
  const boundary = (index: number) => {
    const code = value.charCodeAt(index),
      previous = value.charCodeAt(index - 1);
    return code >= 0xdc00 && code <= 0xdfff && previous >= 0xd800 && previous <= 0xdbff
      ? index - 1
      : index;
  };
  return {
    current,
    last: pages - 1,
    start: boundary(current * RECORDED_TEXT_PAGE_SIZE),
    end: boundary(Math.min((current + 1) * RECORDED_TEXT_PAGE_SIZE, value.length)),
  };
}
