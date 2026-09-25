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
export interface EnvironmentStore {
  list(signal?: AbortSignal): Promise<{ environments: EnvironmentSummary[] }>;
  load(id: string, signal?: AbortSignal): Promise<Environment>;
  save(
    value: EnvironmentDraft & { expectedRevision: string | null },
    signal?: AbortSignal
  ): Promise<Environment>;
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
    list: (signal) => api(endpoint(), undefined, signal),
    load: (id, signal) => api(endpoint(id), undefined, signal),
    save: (value, signal) => api(endpoint(), value, signal, headers()),
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
