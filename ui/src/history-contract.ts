import { ApiError } from './api';
import type { HistoryPage, RuntimeFailure } from './run-history';

export function historyCursorSequence(cursor: unknown): bigint {
  if (typeof cursor !== 'string' || !/^v2:(0|[1-9][0-9]*)$/.test(cursor))
    throw new Error('History contains a gap or inconsistent cursor. Reload to try again.');
  const sequence = BigInt(cursor.slice(3));
  if (sequence > 9223372036854775807n)
    throw new Error('History contains a gap or inconsistent cursor. Reload to try again.');
  return sequence;
}

export function invalidRuntimeFailure(): ApiError {
  return new ApiError(
    200,
    'invalid_runtime_failure',
    'Runtime failure status is inconsistent with retained history. Reload to try again.'
  );
}

export function readRuntimeFailure(value: unknown, head: unknown): RuntimeFailure | undefined {
  if (value === undefined) return undefined;
  const failure = value as Partial<RuntimeFailure> | null;
  try {
    if (
      failure?.reason !== 'runtime_failed' ||
      historyCursorSequence(failure.atCursor) > historyCursorSequence(head)
    )
      throw invalidRuntimeFailure();
  } catch {
    throw invalidRuntimeFailure();
  }
  return failure as RuntimeFailure;
}

export function readPageRuntimeFailure(
  page: Pick<HistoryPage, 'runtimeFailure' | 'headCursor' | 'finished'>
): RuntimeFailure | undefined {
  const failure = readRuntimeFailure(page.runtimeFailure, page.headCursor);
  if (failure && page.finished !== true) throw invalidRuntimeFailure();
  return failure;
}
