import { classifyProviderError } from './adapters';
import type { ProcessResult } from './process-runner';
import type { ErrorClassification, ProviderAdapter } from './types';

export function providerFailureClassification(
  adapter: ProviderAdapter,
  result: ProcessResult
): ErrorClassification | null {
  if (result.timedOut) {
    return classifyProviderError(adapter.id, {
      message: `Provider timed out after ${result.timeoutMs ?? 'unknown'}ms`,
    });
  }
  if (result.exitCode === 0 && result.signal === null) return null;
  return classifyProviderError(adapter.id, {
    message: result.stderr || result.stdout || `Provider exited with code ${result.exitCode ?? ''}`,
  });
}
