import { unlink } from 'node:fs/promises';
import { commandRedactions } from './contract-env';
import { successEnvelope, type ContractEnvelope } from './contract-envelope';
import { buildCommandSpec, optionalNumber, schemaMode, type RequestData } from './contract-support';
import { providerFailureClassification } from './invoke-evidence';
import { parseOutputEvents } from './contract-parse';
import { unknownToMessage } from './json';
import type { CommandSpec } from './types';
import type { ProcessResult, ProcessRunner, ProcessRunnerOptions } from './process-runner';

interface CleanupResult {
  readonly path: string;
  readonly removed: boolean;
  readonly error?: string;
}

async function cleanupFiles(commandSpec: CommandSpec): Promise<readonly CleanupResult[]> {
  const cleanup = commandSpec.cleanup ?? [];
  const results: CleanupResult[] = [];
  for (const file of cleanup) {
    try {
      await unlink(file);
      results.push({ path: file, removed: true });
    } catch (error) {
      results.push({ path: file, removed: false, error: unknownToMessage(error) });
    }
  }
  return results;
}

async function runAndCleanup(
  commandSpec: CommandSpec,
  runner: ProcessRunner,
  runnerOptions: ProcessRunnerOptions
): Promise<{ readonly result: ProcessResult; readonly cleanup: readonly CleanupResult[] }> {
  let result: ProcessResult | null = null;
  let runnerError: unknown;
  try {
    result = await runner(commandSpec, runnerOptions);
  } catch (error) {
    runnerError = error;
  }

  const cleanup = await cleanupFiles(commandSpec);
  if (runnerError !== undefined) throw runnerError;
  if (result === null) throw new Error('Provider runner did not produce a result.');
  return { result, cleanup };
}

export async function runInvoke(
  request: RequestData,
  runner: ProcessRunner
): Promise<ContractEnvelope> {
  const { adapter, commandSpec, options } = buildCommandSpec(request);
  const timeoutMs = optionalNumber(request.raw, 'timeoutMs');
  const runnerOptions = timeoutMs === undefined ? {} : { timeoutMs };
  const { result, cleanup } = await runAndCleanup(commandSpec, runner, runnerOptions);
  const parsed = parseOutputEvents(adapter, {
    chunk: [result.stdout, result.stderr].join('\n'),
    sources: [
      { name: 'stdout', value: result.stdout },
      { name: 'stderr', value: result.stderr },
    ],
  });
  const classification = providerFailureClassification(adapter, result);
  return successEnvelope({
    command: request.command ?? 'invoke',
    adapter,
    warnings: commandSpec.warnings,
    redactions: commandRedactions(commandSpec),
    evidence: invokeEvidence(result, timeoutMs),
    result: {
      commandSpec,
      outputFormat: options.outputFormat ?? null,
      schemaMode: schemaMode(options),
      evidence: {
        stdout: result.stdout,
        stderr: result.stderr,
      },
      events: parsed.events,
      diagnostics: parsed.diagnostics,
      exitCode: result.exitCode,
      signal: result.signal,
      durationMs: result.durationMs,
      timedOut: result.timedOut ?? false,
      timeoutMs: result.timeoutMs ?? timeoutMs ?? null,
      cleanup,
      classification,
    },
  });
}

function invokeEvidence(
  result: ProcessResult,
  timeoutMs: number | undefined
): Record<string, unknown> {
  return {
    exitCode: result.exitCode,
    signal: result.signal,
    durationMs: result.durationMs,
    timedOut: result.timedOut ?? false,
    timeoutMs: result.timeoutMs ?? timeoutMs ?? null,
  };
}
