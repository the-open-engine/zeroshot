import { ApiError, type ApiClient } from './api';

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

export function createEnvironmentStore(api: ApiClient, identity: () => string): EnvironmentStore {
  const headers = () => ({ 'X-Zeroshot-Workspace': identity() });
  return {
    list: (signal) => api('environments', undefined, signal),
    load: (id, signal) => api(`environments/${encodeURIComponent(id)}`, undefined, signal),
    save: async (value, signal) => api('environments', value, signal, headers()),
    remove: async (id, expectedRevision, signal) => {
      const result = await api<{ deleted: boolean }>(
        `environments/${encodeURIComponent(id)}`,
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
