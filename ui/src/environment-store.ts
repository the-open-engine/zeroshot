import { ApiError, createApiClient } from './api';

export type EnvironmentDefinition = {
  setup?: string;
  startup?: string;
  variables?: Record<string, string>;
  connections?: Record<string, string[]>;
};
export type EnvironmentSummary = { id: string; name: string; revision: string };
export type Environment = EnvironmentSummary & { definition: EnvironmentDefinition };
export type EnvironmentDraft = { id?: string; name: string; definition: EnvironmentDefinition };
export type EnvironmentSaveSnapshot = EnvironmentDraft & { expectedRevision: string | null };
export interface EnvironmentStore {
  list(signal?: AbortSignal): Promise<{ environments: EnvironmentSummary[] }>;
  load(id: string, signal?: AbortSignal): Promise<Environment>;
  remove(id: string, expectedRevision: string, signal?: AbortSignal): Promise<void>;
}

/** Hosted storage is explicit; local profile services have no environment catalog. */
export function createEnvironmentStore(
  collection: URL,
  workspaceId: string,
  fetcher: typeof fetch
): EnvironmentStore {
  const api = createApiClient(collection, fetcher);
  const headers = () => ({ 'X-Zeroshot-Workspace': workspaceId });
  function endpoint(id?: string) {
    const url = new URL(collection);
    if (id !== undefined) url.pathname += `/${encodeURIComponent(id)}`;
    return url.href;
  }
  return {
    list: async (signal) => {
      const result = await api<unknown>(endpoint(), undefined, signal);
      if (!record(result) || !Array.isArray(result.environments)) throw invalidResponse();
      return { environments: result.environments.map(readEnvironmentSummary) };
    },
    load: async (id, signal) => {
      const result = readEnvironment(await api<unknown>(endpoint(id), undefined, signal));
      if (!result) throw invalidResponse();
      return result;
    },
    remove: async (id, expectedRevision, signal) => {
      const result = await api<{ deleted: boolean }>(
        endpoint(id),
        { expectedRevision },
        signal,
        headers(),
        'DELETE'
      );
      if (result.deleted !== true)
        throw new ApiError(
          502,
          'invalid_environment_response',
          'The deletion could not be confirmed.'
        );
    },
  };
}

export function readEnvironment(value: unknown): Environment | undefined {
  if (!record(value) || !isEnvironmentDefinition(value.definition)) return;
  const summary = environmentSummary(value);
  if (!summary) return;
  return { ...summary, definition: value.definition };
}

function readEnvironmentSummary(value: unknown): EnvironmentSummary {
  const summary = environmentSummary(value);
  if (!summary) throw invalidResponse();
  return summary;
}

function environmentSummary(value: unknown): EnvironmentSummary | undefined {
  if (
    !record(value) ||
    !boundedText(value.id) ||
    typeof value.name !== 'string' ||
    !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(value.name) ||
    !boundedText(value.revision)
  )
    return;
  return { id: value.id, name: value.name, revision: value.revision };
}

export function isEnvironmentDefinition(value: unknown): value is EnvironmentDefinition {
  if (!record(value)) return false;
  return Object.entries(value).every(([name, field]) => {
    if (name === 'setup' || name === 'startup') return typeof field === 'string';
    if (name === 'variables')
      return record(field) && Object.values(field).every((entry) => typeof entry === 'string');
    if (name === 'connections')
      return (
        record(field) &&
        Object.values(field).every(
          (entry) => Array.isArray(entry) && entry.every((item) => typeof item === 'string')
        )
      );
    return false;
  });
}

function boundedText(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= 256;
}

function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

function invalidResponse() {
  return new ApiError(502, 'invalid_environment_response', 'The environment response is invalid.');
}
