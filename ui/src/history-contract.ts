import { ApiError } from './api';
import { validFailureContext } from './failure-context';
import type { HistoryPage, RunDetail, RuntimeFailure } from './run-history';

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

export function validateFailureContext(context: unknown, head: unknown) {
  if (context == null) return;
  if (!validFailureContext(context)) throw invalidRuntimeFailure();
  for (const diagnostic of [context.primary, ...(context.recorded ?? [])]) {
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
  page: Pick<HistoryPage, 'runtimeFailure' | 'headCursor' | 'finished'>
): RuntimeFailure | undefined {
  const failure = readRuntimeFailure(page.runtimeFailure, page.headCursor);
  if (failure && page.finished !== true) throw invalidRuntimeFailure();
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
  return (
    !run.example && !observationEnded(run.observation, run.history.complete || !!run.runtimeFailure)
  );
}
