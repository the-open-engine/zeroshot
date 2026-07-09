import { classifyProviderError } from './adapters';
import type { ProcessResult } from './process-runner';
import type { ErrorClassification, OutputEvent, ProviderAdapter, ResultEvent } from './types';

function isFailedResultEvent(event: OutputEvent): event is ResultEvent {
  return event.type === 'result' && event.success === false;
}

function classifyParsedFailure(
  adapter: ProviderAdapter,
  events: readonly OutputEvent[]
): ErrorClassification | null {
  const failedResult = [...events].reverse().find(isFailedResultEvent);
  if (failedResult === undefined) return null;
  return classifyProviderError(adapter.id, {
    message:
      typeof failedResult.error === 'string' && failedResult.error.trim()
        ? failedResult.error
        : 'Provider reported a failed result event.',
  });
}

export function providerFailureClassification(
  adapter: ProviderAdapter,
  result: ProcessResult,
  events: readonly OutputEvent[] = []
): ErrorClassification | null {
  if (result.timedOut) {
    return classifyProviderError(adapter.id, {
      message: `Provider timed out after ${result.timeoutMs ?? 'unknown'}ms`,
    });
  }
  if (result.exitCode === 0 && result.signal === null) {
    return classifyParsedFailure(adapter, events);
  }
  return classifyProviderError(adapter.id, {
    message: result.stderr || result.stdout || `Provider exited with code ${result.exitCode ?? ''}`,
  });
}
