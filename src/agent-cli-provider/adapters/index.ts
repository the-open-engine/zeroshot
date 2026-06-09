import { stripTimestampPrefix } from '../log-prefix';
import type {
  BuildProviderCommandOptions,
  CommandSpec,
  ErrorClassification,
  KnownProviderName,
  LevelOverrides,
  ModelLevel,
  OutputEvent,
  ProviderAdapter,
  ProviderId,
  ResolvedModelSpec,
} from '../types';
import { claudeAdapter } from './claude';
export {
  NO_MESSAGES_RETURNED,
  STREAMING_MODE_ERROR,
  detectProviderFatalError,
  detectProviderStreamingModeError,
  recoverProviderStructuredOutput,
  supportsProviderStructuredOutputRecovery,
  type StreamingModeError,
  type StructuredOutputRecovery,
} from './claude-recovery';
import { codexAdapter } from './codex';
import { geminiAdapter } from './gemini';
import { opencodeAdapter } from './opencode';

const ADAPTERS: Readonly<Record<ProviderId, ProviderAdapter>> = {
  claude: claudeAdapter,
  codex: codexAdapter,
  gemini: geminiAdapter,
  opencode: opencodeAdapter,
};

const PROVIDER_IDS: readonly ProviderId[] = ['claude', 'codex', 'gemini', 'opencode'];

function isOutputEventArray(
  event: OutputEvent | readonly OutputEvent[]
): event is readonly OutputEvent[] {
  return Array.isArray(event);
}

function normalizeProviderName(name: string): ProviderId | string {
  const normalized = name.toLowerCase();
  switch (normalized) {
    case 'anthropic':
    case 'claude':
      return 'claude';
    case 'openai':
    case 'codex':
      return 'codex';
    case 'google':
    case 'gemini':
      return 'gemini';
    case 'opencode':
      return 'opencode';
    default:
      return name;
  }
}

function adapterForProviderId(provider: ProviderId): ProviderAdapter {
  return ADAPTERS[provider];
}

function isProviderId(name: string): name is ProviderId {
  return name === 'claude' || name === 'codex' || name === 'gemini' || name === 'opencode';
}

export function getProviderAdapter(name: KnownProviderName | string): ProviderAdapter {
  const normalized = normalizeProviderName(name || '');
  if (isProviderId(normalized)) return adapterForProviderId(normalized);
  throw new Error(`Unknown provider: ${name}. Valid: ${PROVIDER_IDS.join(', ')}`);
}

export function listProviderAdapters(): readonly ProviderId[] {
  return PROVIDER_IDS;
}

export function buildProviderCommand(
  providerName: KnownProviderName | string,
  context: string,
  options?: BuildProviderCommandOptions
): CommandSpec {
  return getProviderAdapter(providerName).buildCommand(context, options);
}

export function parseProviderChunk(
  providerName: KnownProviderName | string,
  chunk: string
): readonly OutputEvent[] {
  const adapter = getProviderAdapter(providerName || 'claude');
  const state = adapter.createParserState();
  const events: OutputEvent[] = [];
  const lines = chunk.split('\n');

  for (const line of lines) {
    const content = stripTimestampPrefix(line);
    if (!content) continue;
    const event = adapter.parseEvent(content, state);
    if (!event) continue;
    if (isOutputEventArray(event)) {
      events.push(...event);
    } else {
      events.push(event);
    }
  }

  return events;
}

export function resolveModelSpec(
  providerName: KnownProviderName | string,
  level: ModelLevel,
  overrides?: LevelOverrides
): ResolvedModelSpec {
  return getProviderAdapter(providerName).resolveModelSpec(level, overrides);
}

export function classifyProviderError(
  providerName: KnownProviderName | string,
  error: unknown
): ErrorClassification {
  return getProviderAdapter(providerName).classifyError(error);
}
