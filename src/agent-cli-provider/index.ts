export {
  buildProviderCommand,
  classifyProviderError,
  detectProviderFatalError,
  detectProviderStreamingModeError,
  getProviderAdapter,
  listProviderAdapters,
  parseProviderChunk,
  recoverProviderStructuredOutput,
  resolveModelSpec,
  supportsProviderStructuredOutputRecovery,
  NO_MESSAGES_RETURNED,
  STREAMING_MODE_ERROR,
  type StreamingModeError,
  type StructuredOutputRecovery,
} from './adapters';
export {
  providerExecutableSchemaVersion,
  runProviderExecutable,
  type ContractEnvelope,
  type ContractErrorEnvelope,
  type ContractErrorObject,
  type ContractEvidence,
  type ContractSuccessEnvelope,
  type ProviderExecutableCommand,
  type ProviderExecutableOptions,
  type ProviderExecutableResponse,
} from './contract';
export {
  spawnProcessRunner,
  type ProcessResult,
  type ProcessRunner,
  type ProcessRunnerOptions,
} from './process-runner';
export {
  detectRuntimeProviderCliFeatures,
  prepareSingleAgentProviderCommand,
  type PreparedSingleAgentProviderCommand,
  type SingleAgentProviderCommandInput,
} from './single-agent-runtime';

export type {
  AgentCliProviderHelperMetadata,
  BuildProviderCommandOptions,
  CleanupMetadata,
  CliFeatureOverrides,
  ClaudeCliFeatures,
  CodexCliFeatures,
  CommandSpec,
  ErrorClassification,
  ErrorClassificationKind,
  GeminiCliFeatures,
  KnownProviderName,
  LevelModelSpec,
  LevelOverrides,
  ModelCatalogEntry,
  ModelLevel,
  ModelSpec,
  OpencodeCliFeatures,
  OutputEvent,
  OutputFormat,
  ProviderAdapter,
  ProviderAlias,
  ProviderCliFeatures,
  ProviderId,
  RedactionMetadata,
  ResolvedModelSpec,
  ResultEvent,
  TextEvent,
  ThinkingEvent,
  ToolCallEvent,
  ToolResultEvent,
  WarningMetadata,
} from './types';

import type { AgentCliProviderHelperMetadata } from './types';

export const agentCliProviderHelperMetadata: Readonly<AgentCliProviderHelperMetadata> = {
  packageName: '@the-open-engine/zeroshot',
  buildOutputDir: 'lib/agent-cli-provider',
  contractVersion: 1,
  adapterVersion: '1',
};
