import { ApiError } from './api';
import { validFailureContext } from './failure-context';
import type { HistoryPage, RunDetail, RuntimeFailure } from './run-history';

export function historyCursorSequence(cursor: unknown): bigint {
  if (
    typeof cursor === 'string' &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}@/.test(cursor)
  )
    cursor = cursor.slice(cursor.indexOf('@') + 1);
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

export function validateFailureContext(context: unknown, head: unknown) {
  if (context == null) return;
  if (!validFailureContext(context)) throw invalidRuntimeFailure();
  for (const diagnostic of [
    context.primary,
    ...(context.recorded ?? []),
    ...(context.verifiers ?? []),
  ]) {
    if (
      diagnostic?.cursor != null &&
      historyCursorSequence(diagnostic.cursor) > historyCursorSequence(head)
    )
      throw invalidRuntimeFailure();
  }
}

export function readRuntimeFailure(value: unknown, head: unknown): RuntimeFailure | undefined {
  if (value === undefined) return undefined;
  const failure = value as Partial<RuntimeFailure> | null;
  try {
    if (
      (failure?.reason !== 'runtime_failed' && failure?.reason !== 'runtime_lost') ||
      historyCursorSequence(failure.atCursor) > historyCursorSequence(head)
    )
      throw invalidRuntimeFailure();
    validateFailureContext(failure.context, head);
  } catch {
    throw invalidRuntimeFailure();
  }
  return failure as RuntimeFailure;
}

export function readPageRuntimeFailure(
  page: Pick<
    HistoryPage,
    'runtimeFailure' | 'headCursor' | 'finished' | 'partHeadCursor' | 'partId' | 'currentPartId'
  >
): RuntimeFailure | undefined {
  const failure = readRuntimeFailure(page.runtimeFailure, page.partHeadCursor ?? page.headCursor);
  if (failure && page.finished !== true && page.partId === page.currentPartId)
    throw invalidRuntimeFailure();
  return failure;
}

export type HistoryObservation = {
  state: 'active' | 'collecting' | 'complete' | 'incomplete' | 'expired' | 'unavailable';
  code?: string;
};
/** Observation may continue while a terminal run's archive is still collecting. */
export function readObservation(value: unknown): HistoryObservation | undefined {
  if (value === undefined) return undefined;
  const result = value as Partial<HistoryObservation> | null;
  const states = ['active', 'collecting', 'complete', 'incomplete', 'expired', 'unavailable'];
  if (
    !result ||
    !states.includes(result.state ?? '') ||
    (result.code !== undefined && typeof result.code !== 'string')
  )
    throw new ApiError(
      200,
      'invalid_observation',
      'History availability is invalid. Reload to try again.'
    );
  return result as HistoryObservation;
}
export function observationEnded(
  observation: HistoryObservation | undefined,
  fallback: boolean
): boolean {
  return observation ? !['active', 'collecting'].includes(observation.state) : fallback;
}
export function canFollowHistory(run: RunDetail): boolean {
  if (run.phase === 'suspended') return !!run.parts && !run.partView && !run.example;
  return (
    !run.example && !observationEnded(run.observation, run.history.complete || !!run.runtimeFailure)
  );
}

/** The backend transports native u64 identities as strings; accept only lossless legacy numbers. */
export function executionIdentity(value: string | number): string {
  if (typeof value === 'number' && (!Number.isSafeInteger(value) || value < 1)) {
    throw new Error(
      'History contains an imprecise execution identity. Reload with a compatible server.'
    );
  }
  return String(value);
}

export function scopedExecutionId(part: string | undefined, execution: string | number) {
  return part ? `${part}@${executionIdentity(execution)}` : executionIdentity(execution);
}
