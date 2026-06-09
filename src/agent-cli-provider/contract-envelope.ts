import { collectCommandSpecEnv, mergeEnvForRedaction, providerCredentialEnv } from './contract-env';
import { isRecord } from './json';
import { mergeRedactions, redactObject } from './redaction';
import type { ProviderAdapter, RedactionMetadata, WarningMetadata } from './types';

export const providerExecutableSchemaVersion = 1 as const;

export interface ContractErrorObject {
  readonly code: string;
  readonly message: string;
  readonly field?: string;
  readonly classification?: unknown;
}

export interface ContractEvidence {
  readonly [key: string]: unknown;
}

interface ContractEnvelopeBase {
  readonly schemaVersion: 1;
  readonly command: string | null;
  readonly provider: string | null;
  readonly adapterVersion: string | null;
  readonly warnings: readonly WarningMetadata[];
  readonly redactions: readonly RedactionMetadata[];
  readonly evidence: ContractEvidence;
}

export interface ContractSuccessEnvelope<Result = unknown> extends ContractEnvelopeBase {
  readonly ok: true;
  readonly result: Result;
}

export interface ContractErrorEnvelope extends ContractEnvelopeBase {
  readonly ok: false;
  readonly error: ContractErrorObject;
}

export type ContractEnvelope<Result = unknown> =
  | ContractSuccessEnvelope<Result>
  | ContractErrorEnvelope;

export function finalizeEnvelope(
  envelope: ContractEnvelope,
  env: Readonly<Record<string, string>>
): ContractEnvelope {
  const redacted = redactObject(
    envelope,
    mergeEnvForRedaction(
      { source: 'commandSpec', env: collectCommandSpecEnv(envelope) },
      { source: 'process', env: providerCredentialEnv(envelope.provider) },
      { source: 'request', env }
    )
  );
  if (!isContractEnvelope(redacted.value)) {
    throw new Error('Redacted provider contract envelope lost its JSON shape.');
  }
  return {
    ...redacted.value,
    redactions: mergeRedactions(envelope.redactions, redacted.redactions),
  };
}

function isContractEnvelope(value: unknown): value is ContractEnvelope {
  if (!isRecord(value)) return false;
  return value.schemaVersion === providerExecutableSchemaVersion && typeof value.ok === 'boolean';
}

export function successEnvelope<Result>(input: {
  readonly command: string;
  readonly adapter: ProviderAdapter;
  readonly result: Result;
  readonly warnings?: readonly WarningMetadata[];
  readonly redactions?: readonly RedactionMetadata[];
  readonly evidence?: ContractEvidence;
}): ContractSuccessEnvelope<Result> {
  return {
    schemaVersion: providerExecutableSchemaVersion,
    ok: true,
    command: input.command,
    provider: input.adapter.id,
    adapterVersion: input.adapter.adapterVersion,
    warnings: input.warnings ?? [],
    redactions: input.redactions ?? [],
    evidence: input.evidence ?? {},
    result: input.result,
  };
}

export function errorEnvelope(input: {
  readonly command: string | null;
  readonly provider: string | null;
  readonly adapterVersion?: string | null;
  readonly error: ContractErrorObject;
  readonly warnings?: readonly WarningMetadata[];
  readonly redactions?: readonly RedactionMetadata[];
  readonly evidence?: ContractEvidence;
}): ContractErrorEnvelope {
  return {
    schemaVersion: providerExecutableSchemaVersion,
    ok: false,
    command: input.command,
    provider: input.provider,
    adapterVersion: input.adapterVersion ?? null,
    warnings: input.warnings ?? [],
    redactions: input.redactions ?? [],
    evidence: input.evidence ?? {},
    error: input.error,
  };
}
