import type {
  providerAliases,
  providerIds,
  ProviderCapabilities,
  ProviderCapabilityState,
  ProviderCommandSpec,
  ProviderDockerMetadata,
  ProviderDockerMountPreset,
  ProviderDocsMetadata,
  ProviderRegistryEntry,
} from './provider-registry';

export type ProviderId = (typeof providerIds)[number];
export type ProviderAlias = (typeof providerAliases)[number];
export type KnownProviderName = ProviderId | ProviderAlias;
export type ModelLevel = 'level1' | 'level2' | 'level3';
export type ReasoningEffort = 'low' | 'medium' | 'high' | 'xhigh';
export type OutputFormat = 'text' | 'json' | 'stream-json';
export type {
  ProviderCapabilities,
  ProviderCapabilityState,
  ProviderCommandSpec,
  ProviderDockerMetadata,
  ProviderDockerMountPreset,
  ProviderDocsMetadata,
  ProviderRegistryEntry,
};

export interface AgentCliProviderHelperMetadata {
  readonly packageName: '@the-open-engine/zeroshot';
  readonly buildOutputDir: 'lib/agent-cli-provider';
  readonly contractVersion: 1;
  readonly adapterVersion: string;
}

export interface ModelCatalogEntry {
  readonly rank: number;
}

export interface LevelModelSpec {
  readonly rank: number;
  readonly model: string | null;
  readonly reasoningEffort?: ReasoningEffort;
}

export interface ModelSpec {
  readonly level?: ModelLevel;
  readonly model?: string | null;
  readonly reasoningEffort?: ReasoningEffort;
}

export interface ResolvedModelSpec {
  readonly level: ModelLevel;
  readonly model: string | null;
  readonly reasoningEffort: ReasoningEffort | undefined;
}

export type LevelOverrides = Readonly<Partial<Record<ModelLevel, ModelSpec>>>;

export interface BaseCliFeatures {
  readonly provider?: ProviderId;
  readonly unknown?: boolean;
}

export interface ClaudeCliFeatures extends BaseCliFeatures {
  readonly provider: 'claude';
  readonly supportsOutputFormat: boolean;
  readonly supportsStreamJson: boolean;
  readonly supportsJsonSchema: boolean;
  readonly supportsAutoApprove: boolean;
  readonly supportsIncludePartials: boolean;
  readonly supportsVerbose: boolean;
  readonly supportsModel: boolean;
}

export interface CodexCliFeatures extends BaseCliFeatures {
  readonly provider: 'codex';
  readonly supportsJson: boolean;
  readonly supportsOutputSchema: boolean;
  readonly supportsAutoApprove: boolean;
  readonly supportsCwd: boolean;
  readonly supportsConfigOverride: boolean;
  readonly supportsModel: boolean;
  readonly supportsSkipGitRepoCheck: boolean;
}

export interface GeminiCliFeatures extends BaseCliFeatures {
  readonly provider: 'gemini';
  readonly supportsStreamJson: boolean;
  readonly supportsAutoApprove: boolean;
  readonly supportsCwd: boolean;
  readonly supportsModel: boolean;
}

export interface OpencodeCliFeatures extends BaseCliFeatures {
  readonly provider: 'opencode';
  readonly supportsJson: boolean;
  readonly supportsModel: boolean;
  readonly supportsVariant: boolean;
  readonly supportsDir: boolean;
  readonly supportsCwd: boolean;
  readonly supportsAutoApprove: false;
}

export interface PiCliFeatures extends BaseCliFeatures {
  readonly provider: 'pi';
  readonly supportsJsonMode: boolean;
  readonly supportsModel: boolean;
  readonly supportsNoSession: boolean;
  readonly supportsNoExtensions: boolean;
  readonly supportsNoSkills: boolean;
  readonly supportsNoPromptTemplates: boolean;
  readonly supportsNoContextFiles: boolean;
  readonly supportsNoApprove: boolean;
}

export type ProviderCliFeatures =
  | ClaudeCliFeatures
  | CodexCliFeatures
  | GeminiCliFeatures
  | OpencodeCliFeatures
  | PiCliFeatures;

export interface CliFeatureOverrides {
  readonly supportsOutputFormat?: boolean;
  readonly supportsStreamJson?: boolean;
  readonly supportsJsonSchema?: boolean;
  readonly supportsAutoApprove?: boolean;
  readonly supportsIncludePartials?: boolean;
  readonly supportsVerbose?: boolean;
  readonly supportsModel?: boolean;
  readonly supportsJson?: boolean;
  readonly supportsOutputSchema?: boolean;
  readonly supportsDir?: boolean;
  readonly supportsCwd?: boolean;
  readonly supportsConfigOverride?: boolean;
  readonly supportsSkipGitRepoCheck?: boolean;
  readonly supportsVariant?: boolean;
  readonly supportsJsonMode?: boolean;
  readonly supportsNoSession?: boolean;
  readonly supportsNoExtensions?: boolean;
  readonly supportsNoSkills?: boolean;
  readonly supportsNoPromptTemplates?: boolean;
  readonly supportsNoContextFiles?: boolean;
  readonly supportsNoApprove?: boolean;
  readonly unknown?: boolean;
}

export interface CleanupMetadata {
  readonly kind: 'temp-file';
  readonly provider: ProviderId;
  readonly path: string;
  readonly reason: 'output-schema';
}

export interface WarningMetadata {
  readonly provider: ProviderId;
  readonly code: string;
  readonly message: string;
}

export interface RedactionMetadata {
  readonly kind: 'env' | 'secret-key' | 'secret-value';
  readonly key: string;
  readonly source?: string;
}

export interface CommandSpec {
  readonly binary: string;
  readonly args: readonly string[];
  readonly env: Readonly<Record<string, string>>;
  readonly cwd?: string;
  readonly cleanup?: readonly string[];
  readonly cleanupMetadata: readonly CleanupMetadata[];
  readonly warnings: readonly WarningMetadata[];
  readonly redactions: readonly RedactionMetadata[];
}

export interface BuildProviderCommandOptions {
  readonly modelSpec?: ModelSpec;
  readonly outputFormat?: OutputFormat;
  readonly jsonSchema?: unknown;
  readonly cwd?: string;
  readonly autoApprove?: boolean;
  readonly resumeSessionId?: string;
  readonly continueSession?: boolean;
  readonly cliFeatures?: CliFeatureOverrides;
  readonly authEnv?: Readonly<Record<string, string>>;
  readonly strictSchema?: boolean;
}

export interface TextEvent {
  readonly type: 'text';
  readonly text: string;
}

export interface ThinkingEvent {
  readonly type: 'thinking';
  readonly text: string;
}

export interface ToolCallEvent {
  readonly type: 'tool_call';
  readonly toolName: string | null | undefined;
  readonly toolId: string | null | undefined;
  readonly input: unknown;
}

export interface ToolResultEvent {
  readonly type: 'tool_result';
  readonly toolId: string | null | undefined;
  readonly content: unknown;
  readonly isError: unknown;
}

export interface ResultEvent {
  readonly type: 'result';
  readonly success: boolean;
  readonly result?: unknown;
  readonly error?: unknown;
  readonly cost?: unknown;
  readonly duration?: unknown;
  readonly inputTokens?: number;
  readonly outputTokens?: number;
  readonly cacheReadInputTokens?: number;
  readonly cacheCreationInputTokens?: number;
  readonly modelUsage?: unknown;
}

export type OutputEvent = TextEvent | ThinkingEvent | ToolCallEvent | ToolResultEvent | ResultEvent;

export type ProviderParseResult = OutputEvent | readonly OutputEvent[] | null;

export interface ProviderParserState {
  readonly provider: ProviderId;
  lastToolId: string | null | undefined;
  lastAssistantText?: string;
  lastAssistantThinking?: string;
}

export type ErrorClassificationKind =
  | 'status-retryable'
  | 'status-permanent'
  | 'code-retryable'
  | 'permanent-pattern'
  | 'retryable-pattern'
  | 'unknown-retryable';

export interface ErrorClassification {
  readonly retryable: boolean;
  readonly kind: ErrorClassificationKind;
  readonly matchedPattern?: string;
}

export interface ProviderAdapter {
  readonly id: ProviderId;
  readonly displayName: string;
  readonly binary: string;
  readonly adapterVersion: string;
  readonly credentialEnvKeys: readonly string[];
  readonly modelCatalog: Readonly<Record<string, ModelCatalogEntry>>;
  readonly levelMapping: Readonly<Record<ModelLevel, LevelModelSpec>>;
  readonly defaultLevel: ModelLevel;
  readonly defaultMaxLevel: ModelLevel;
  readonly defaultMinLevel: ModelLevel;
  detectCliFeatures(helpText?: string | null): ProviderCliFeatures;
  buildCommand(context: string, options?: BuildProviderCommandOptions): CommandSpec;
  parseEvent(line: string, state: ProviderParserState): ProviderParseResult;
  createParserState(): ProviderParserState;
  resolveModelSpec(level: ModelLevel, overrides?: LevelOverrides): ResolvedModelSpec;
  validateModelId(modelId: string | null | undefined): string | null | undefined;
  classifyError(error: unknown): ErrorClassification;
}

export class InvalidProviderModelError extends Error {
  readonly permanent = true;

  constructor(message: string) {
    super(message);
    this.name = 'InvalidProviderModelError';
  }
}
