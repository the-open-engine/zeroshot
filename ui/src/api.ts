import type { WorkerOption } from './workers';
import type { Document, Template } from './domain';
export type Summary = { id: string; name: string; isDefault: boolean };
export type Saved = { profile: Document & Summary; revision: string };
export type ApiClient = <T>(
  path: string,
  body?: unknown,
  signal?: AbortSignal,
  headers?: Record<string, string>
) => Promise<T>;

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly details?: unknown
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

/** Only this adapter knows the browser API's mount point and HTTP error envelope. */
export function createApiClient(base: URL, fetcher: typeof fetch = fetch): ApiClient {
  return async <T>(
    path: string,
    body?: unknown,
    signal?: AbortSignal,
    headers?: Record<string, string>
  ): Promise<T> => {
    const response = await fetcher(new URL(path, base), {
      method: body === undefined ? 'GET' : 'POST',
      headers: {
        ...headers,
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal,
      cache: 'no-store',
    });
    const data = await response.json().catch(() => null);
    if (!response.ok)
      throw new ApiError(
        response.status,
        typeof data?.code === 'string' ? data.code : 'request_failed',
        typeof data?.message === 'string' ? data.message : `Request failed (${response.status}).`,
        data?.details
      );
    if (data === null)
      throw new ApiError(
        response.status,
        'invalid_response',
        'The server returned an invalid response.'
      );
    return data as T;
  };
}

export type WorkspaceIdentity = { kind: 'local' | 'target'; id: string };
export type Bootstrap = {
  templates: Template[];
  workers?: WorkerOption[];
  runtimeSchema: any;
  version: number;
  workspace: WorkspaceIdentity;
};

export function readBootstrap(value: unknown): Bootstrap {
  const result = value as Partial<Bootstrap> | null;
  if (result?.version !== 1)
    throw new ApiError(
      200,
      'incompatible_ui',
      'This UI and server use different interface versions. Reload after updating Zeroshot.'
    );
  if (
    !Array.isArray(result.templates) ||
    !result.runtimeSchema ||
    !['local', 'target'].includes(result.workspace?.kind ?? '') ||
    typeof result.workspace?.id !== 'string' ||
    !result.workspace.id.trim()
  )
    throw new ApiError(200, 'invalid_bootstrap', 'The server did not identify its workspace.');
  return result as Bootstrap;
}
