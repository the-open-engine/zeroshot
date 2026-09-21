import { ApiError } from './api';
import type { WorkspaceInit } from './workspace-bridge';

/** Read the existing same-origin CSRF cookie per write; no token crosses the host bridge. */
export function createEmbeddedFetch(
  base: URL,
  csrf: WorkspaceInit['csrf'],
  cookie: () => string = () => document.cookie,
  fetcher: typeof fetch = fetch
): typeof fetch {
  return async (input, init) => {
    const url = new URL(input instanceof Request ? input.url : String(input), base);
    if (url.origin !== base.origin || !url.pathname.startsWith(base.pathname))
      throw new ApiError(403, 'foreign_service', 'Workspace request is outside its service mount.');
    const method = (
      init?.method ?? (input instanceof Request ? input.method : 'GET')
    ).toUpperCase();
    const request = { ...init, redirect: 'error' as const, credentials: 'same-origin' as const };
    if (!csrf || method !== 'POST') return fetcher(input instanceof Request ? input : url, request);
    const token = csrfToken(cookie(), csrf.cookieName);
    const headers = new Headers(
      init?.headers ?? (input instanceof Request ? input.headers : undefined)
    );
    headers.set(csrf.headerName, token);
    return fetcher(input instanceof Request ? input : url, { ...request, headers });
  };
}

function csrfToken(cookie: string, name: string): string {
  const tokens = cookie
    .split(';')
    .map((part) => part.trim())
    .filter((part) => part.slice(0, part.indexOf('=')) === name);
  if (tokens.length !== 1)
    throw new ApiError(403, 'csrf_missing', 'Reopen the workspace to refresh its session.');
  const token = tokens[0].slice(tokens[0].indexOf('=') + 1);
  if (!token || !/^[A-Za-z0-9_-]+$/.test(token))
    throw new ApiError(403, 'csrf_invalid', 'Reopen the workspace to refresh its session.');
  return token;
}
