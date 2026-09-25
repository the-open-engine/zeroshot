import {
  ApiError,
  createApiClient,
  readBootstrap,
  type Bootstrap,
  type Saved,
  type Summary,
} from './api';
import type { Document } from './domain';
import type { NodeDataAction } from './node-data';
import { createRunHistorySource, type RunHistorySource } from './run-history-source';

export type ProfileContent = Pick<Document, 'graph' | 'runtime'>;
export type SaveProfile = ProfileContent & { name: string; expectedRevision: string | null };
export interface ProfileStore {
  list(signal?: AbortSignal): Promise<{ profiles: Summary[] }>;
  load(name: string, signal?: AbortSignal): Promise<Saved>;
  save(profile: SaveProfile, signal?: AbortSignal): Promise<Saved>;
}
export type AuthoringAction =
  | { kind: 'failure_reason'; terminal: string; reason: string }
  | { kind: 'complete'; owner: string }
  | { kind: 'protect'; node: string };
export interface AuthoringService {
  validate(document: ProfileContent, signal?: AbortSignal): Promise<void>;
  outcome(document: Document, action: AuthoringAction, signal?: AbortSignal): Promise<Document>;
  data(document: Document, action: NodeDataAction, signal?: AbortSignal): Promise<Document>;
}
export interface WorkspaceServices {
  readonly mount: URL;
  catalog(signal?: AbortSignal): Promise<Bootstrap>;
  profiles: ProfileStore;
  authoring: AuthoringService;
  history: RunHistorySource;
}

/** The standalone shell composes services; graph components never choose storage or URLs. */
export function createWorkspaceServices(
  mount: URL,
  fetcher: typeof fetch = fetch,
  apiBase?: URL
): WorkspaceServices {
  const base = apiBase ?? new URL('./api/', mount);
  if (base.origin !== mount.origin)
    throw new ApiError(403, 'foreign_service', 'Workspace services must use the host origin.');
  const api = createApiClient(base, fetcher);
  let workspaceId: string | undefined;
  async function author(
    path: string,
    document: Document,
    action: AuthoringAction | NodeDataAction,
    signal?: AbortSignal
  ): Promise<Document> {
    const response = await api<ProfileContent>(
      path,
      {
        graph: document.graph,
        runtime: document.runtime,
        action,
      },
      signal
    );
    return { ...document, ...response };
  }
  return {
    mount,
    catalog: async (signal) => {
      const value = readBootstrap(await api('bootstrap', undefined, signal));
      if (workspaceId !== undefined && value.workspace.id !== workspaceId)
        throw new ApiError(
          409,
          'workspace_changed',
          'This workspace changed. Reload before saving. Your draft is unchanged.'
        );
      workspaceId ??= value.workspace.id;
      return value;
    },
    profiles: {
      list: (signal) => api('profiles', undefined, signal),
      load: (name, signal) => api(`profiles/${encodeURIComponent(name)}`, undefined, signal),
      save: async (profile, signal) => {
        if (workspaceId === undefined)
          throw new ApiError(
            409,
            'workspace_required',
            'Reload Zeroshot to identify the workspace before saving.'
          );
        return api('profiles', profile, signal, { 'X-Zeroshot-Workspace': workspaceId });
      },
    },
    authoring: {
      validate: async (document, signal) => {
        await api('validate', document, signal);
      },
      outcome: (document, action, signal) => author('authoring', document, action, signal),
      data: (document, action, signal) => author('data', document, action, signal),
    },
    history: createRunHistorySource(api, base, fetcher),
  };
}
