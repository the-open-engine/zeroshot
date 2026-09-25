import { assertDocument, type Document } from './domain';
import type { Saved } from './api';

export type WorkspaceAuthority = { userId: string; organizationId: string; scope: 'user' | 'org' };
type Envelope = {
  version: 1;
  workspaceId: string;
  requestId: string;
  documentId: string | null;
  generation: number;
};
export type WorkspaceInit = {
  version: 1;
  type: 'init';
  requestId: string;
  workspaceId: string;
  authority: WorkspaceAuthority;
  apiBase: string;
  theme: 'light' | 'dark';
  view?: 'profiles' | 'environments';
  readOnly?: boolean;
  csrf?: { cookieName: string; headerName: string };
};
export type HostCommand = Envelope &
  (
    | ({
        type: 'open_profile';
        nextDocumentId: string;
        discard?: boolean;
      } & (
        | { profile: Document; revision?: string; templateId?: never }
        | { templateId: string; profile?: never; revision?: never }
      ))
    | { type: 'open_run'; nextDocumentId: string; runId: string; discard?: boolean }
    | { type: 'request_save'; name?: string }
    | {
        type: 'save_ack';
        saved?: Saved;
        problem?: { code: string; message: string; details?: unknown };
      }
    | { type: 'theme'; theme: 'light' | 'dark' }
    | { type: 'navigate'; action: 'profiles' | 'runs' | 'leave'; discard?: boolean }
  );
type BridgeParent = { postMessage(message: unknown, targetOrigin: string): void };
type Listener = (event: Pick<MessageEvent, 'source' | 'origin' | 'data'>) => void;
const token = (value: unknown): value is string =>
  typeof value === 'string' && value.length > 0 && value.length <= 256;
const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

/** The trusted host supplies authority and services; messages never carry history or credentials. */
export class WorkspaceBridge {
  readonly readyId = crypto.randomUUID();
  configuration?: WorkspaceInit;
  private disposed = false;
  private handler?: (command: HostCommand) => void;
  private readonly unlisten: () => void;
  constructor(
    private readonly origin: string,
    private readonly parent: BridgeParent,
    listen: (listener: Listener) => () => void,
    private readonly initialized: (value: WorkspaceInit) => void
  ) {
    this.unlisten = listen((event) => this.receive(event));
    parent.postMessage({ version: 1, type: 'ready', requestId: this.readyId }, origin);
  }
  connect(handler: (command: HostCommand) => void): () => void {
    this.handler = handler;
    return () => {
      if (this.handler === handler) this.handler = undefined;
    };
  }
  send(
    type: 'save_snapshot' | 'state' | 'navigate',
    payload: Record<string, unknown>,
    identity: {
      documentId: string | null;
      generation: number;
      requestId?: string;
    }
  ) {
    if (!this.configuration || this.disposed) return;
    this.parent.postMessage(
      {
        ...payload,
        version: 1,
        type,
        workspaceId: this.configuration.workspaceId,
        documentId: identity.documentId,
        generation: identity.generation,
        requestId: identity.requestId ?? crypto.randomUUID(),
      },
      this.origin
    );
  }
  dispose() {
    this.disposed = true;
    this.handler = undefined;
    this.unlisten();
  }
  private receive(event: Pick<MessageEvent, 'source' | 'origin' | 'data'>) {
    if (this.disposed || event.origin !== this.origin || event.source !== this.parent) return;
    const value: unknown = event.data;
    if (!bounded(value)) return;
    if (!this.configuration) {
      const init = readInit(value, this.origin, this.readyId);
      if (init) {
        this.configuration = init;
        this.initialized(init);
      }
      return;
    }
    const command = readCommand(value, this.configuration.workspaceId);
    if (command) this.handler?.(command);
  }
}
function readInit(
  value: Record<string, unknown>,
  origin: string,
  requestId: string
): WorkspaceInit | undefined {
  if (
    value.version !== 1 ||
    value.type !== 'init' ||
    value.requestId !== requestId ||
    !token(value.workspaceId) ||
    !record(value.authority)
  )
    return;
  const authority = value.authority;
  if (
    !token(authority.userId) ||
    !token(authority.organizationId) ||
    !['user', 'org'].includes(String(authority.scope)) ||
    !['light', 'dark'].includes(String(value.theme))
  )
    return;
  if (!validCsrf(value.csrf)) return;
  if (value.view !== undefined && !['profiles', 'environments'].includes(String(value.view)))
    return;
  if (value.readOnly !== undefined && typeof value.readOnly !== 'boolean') return;
  if (typeof value.apiBase !== 'string' || !value.apiBase.startsWith('/')) return;
  let base: URL;
  try {
    base = new URL(value.apiBase, origin);
  } catch {
    return;
  }
  if (
    base.origin !== origin ||
    base.username ||
    base.password ||
    base.search ||
    base.hash ||
    !base.pathname.endsWith('/')
  )
    return;
  return value as unknown as WorkspaceInit;
}
function invalidCommandEnvelope(value: Record<string, unknown>, workspaceId: string): boolean {
  return (
    value.version !== 1 ||
    value.workspaceId !== workspaceId ||
    !token(value.requestId) ||
    !(value.documentId === null || token(value.documentId)) ||
    !Number.isSafeInteger(value.generation) ||
    Number(value.generation) < 0 ||
    (value.discard !== undefined && typeof value.discard !== 'boolean')
  );
}
function readCommand(value: Record<string, unknown>, workspaceId: string): HostCommand | undefined {
  if (invalidCommandEnvelope(value, workspaceId)) return;
  switch (value.type) {
    case 'open_profile':
      return readProfileCommand(value);
    case 'open_run':
      return token(value.nextDocumentId) && token(value.runId) ? (value as HostCommand) : undefined;
    case 'request_save':
      return value.name === undefined || token(value.name) ? (value as HostCommand) : undefined;
    case 'save_ack':
      return readAcknowledgement(value);
    case 'theme':
      return ['light', 'dark'].includes(String(value.theme)) ? (value as HostCommand) : undefined;
    case 'navigate':
      return ['profiles', 'runs', 'leave'].includes(String(value.action))
        ? (value as HostCommand)
        : undefined;
    default:
      return;
  }
}
function readProfileCommand(value: Record<string, unknown>): HostCommand | undefined {
  if (!token(value.nextDocumentId) || (value.revision !== undefined && !token(value.revision)))
    return;
  if (value.templateId !== undefined) {
    return token(value.templateId) && value.profile === undefined && value.revision === undefined
      ? (value as HostCommand)
      : undefined;
  }
  try {
    assertDocument(value.profile);
  } catch {
    return;
  }
  return value as HostCommand;
}
function readAcknowledgement(value: Record<string, unknown>): HostCommand | undefined {
  if (
    record(value.problem) &&
    token(value.problem.code) &&
    typeof value.problem.message === 'string' &&
    !value.saved
  )
    return value as HostCommand;
  if (!record(value.saved) || value.problem || !token(value.saved.revision)) return;
  try {
    assertDocument(value.saved.profile);
  } catch {
    return;
  }
  return value as HostCommand;
}

function validCsrf(value: unknown): boolean {
  if (value === undefined) return true;
  return (
    record(value) &&
    typeof value.cookieName === 'string' &&
    /^[A-Za-z0-9_-]{1,80}$/.test(value.cookieName) &&
    value.headerName === 'X-Zeroshot-CSRF'
  );
}
function bounded(value: unknown): value is Record<string, unknown> {
  if (!record(value)) return false;
  try {
    return JSON.stringify(value).length <= 2 * 1024 * 1024;
  } catch {
    return false;
  }
}
