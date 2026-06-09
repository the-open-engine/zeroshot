import { classifyProviderError } from './adapters';
import { envRedactions, commandRedactions } from './contract-env';
import { contractError } from './contract-errors';
import {
  providerExecutableSchemaVersion,
  successEnvelope,
  type ContractEnvelope,
  type ContractEvidence,
} from './contract-envelope';
import { runInvoke } from './contract-invoke';
import { runParseOutput } from './contract-parse';
import {
  adapterForProvider,
  buildCommandSpec,
  schemaMode,
  type RequestData,
} from './contract-support';
import { detectRuntimeProviderCliFeatures } from './single-agent-runtime';
import { getNumber, getRecord, getString, isRecord, unknownToMessage } from './json';
import type { ErrorClassification } from './types';
import type { ProcessRunner } from './process-runner';

function runBuildCommand(request: RequestData): ContractEnvelope {
  const { adapter, commandSpec, options } = buildCommandSpec(request);
  return successEnvelope({
    command: request.command ?? 'build-command',
    adapter,
    warnings: commandSpec.warnings,
    redactions: commandRedactions(commandSpec),
    evidence: {
      outputFormat: options.outputFormat ?? null,
      schemaMode: schemaMode(options),
    },
    result: {
      commandSpec,
      outputFormat: options.outputFormat ?? null,
      schemaMode: schemaMode(options),
    },
  });
}

function runProbe(request: RequestData): ContractEnvelope {
  const adapter = adapterForProvider(request.provider);
  const helpText = typeof request.raw.helpText === 'string' ? request.raw.helpText : null;
  const capabilities =
    helpText === null ? detectRuntimeProviderCliFeatures(adapter.id) : adapter.detectCliFeatures(helpText);
  return successEnvelope({
    command: request.command ?? 'probe',
    adapter,
    redactions: envRedactions(request.env),
    result: {
      provider: {
        id: adapter.id,
        displayName: adapter.displayName,
        binary: adapter.binary,
      },
      contractVersion: providerExecutableSchemaVersion,
      adapterVersion: adapter.adapterVersion,
      capabilities,
      credentials: adapter.credentialEnvKeys.map((key) => ({
        key,
        present: Boolean(request.env[key] ?? process.env[key]),
      })),
    },
  });
}

function statusFromError(error: unknown): number | null {
  if (!isRecord(error)) return null;
  const directStatus = getNumber(error, 'status') ?? getNumber(error, 'statusCode');
  if (directStatus !== null) return directStatus;

  const response = getRecord(error, 'response');
  if (response === null) return null;
  return getNumber(response, 'status') ?? getNumber(response, 'statusCode');
}

function categoryForClassification(classification: ErrorClassification, error: unknown): string {
  const status = statusFromError(error);
  if (status === 401 || status === 403) return 'auth';
  if (status === 429) return 'rate-limit';
  const message = unknownToMessage(error);
  if (/auth|api[_ -]?key|unauthorized|forbidden|permission/i.test(message)) return 'auth';
  if (/rate|429|quota|resource_exhausted/i.test(message)) return 'rate-limit';
  if (/schema|json|parse|format/i.test(message)) return 'schema';
  return classification.retryable ? 'retryable' : 'permanent';
}

function errorEvidence(error: unknown): ContractEvidence {
  const evidence: Record<string, unknown> = {
    message: unknownToMessage(error),
  };
  if (isRecord(error)) {
    const status = statusFromError(error);
    const code = getString(error, 'code');
    if (status !== null) evidence.status = status;
    if (code !== null) evidence.code = code;
  }
  return evidence;
}

function runClassifyError(request: RequestData): ContractEnvelope {
  const adapter = adapterForProvider(request.provider);
  const error = request.raw.error;
  if (error === undefined) {
    throw contractError({
      code: 'missing-field',
      message: 'error is required.',
      exitCode: 2,
      field: 'error',
    });
  }
  const classification = classifyProviderError(adapter.id, error);
  return successEnvelope({
    command: request.command ?? 'classify-error',
    adapter,
    redactions: envRedactions(request.env),
    evidence: errorEvidence(error),
    result: {
      classification,
      category: categoryForClassification(classification, error),
      evidence: errorEvidence(error),
    },
  });
}

export function dispatchRequest(
  request: RequestData,
  runner: ProcessRunner
): ContractEnvelope | Promise<ContractEnvelope> {
  switch (request.command) {
    case 'probe':
      return runProbe(request);
    case 'build-command':
      return runBuildCommand(request);
    case 'parse-output':
      return runParseOutput(request);
    case 'classify-error':
      return runClassifyError(request);
    case 'invoke':
      return runInvoke(request, runner);
    default:
      throw contractError({
        code: 'unknown-command',
        message: `Unknown command: ${request.command ?? ''}.`,
        exitCode: 3,
        field: 'command',
      });
  }
}
