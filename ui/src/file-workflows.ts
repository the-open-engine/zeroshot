/** File metadata only. The target owns bytes and admission; the embedding host owns authorization. */
export type FileScope = { kind: 'user' | 'org' } | { kind: 'run'; runId: string };
export type FileAddress = { scope: FileScope; path: string };
export type FileSelection = { address: FileAddress; member: string };
export type SeedSelection = FileSelection & {
  address: { scope: { kind: 'user' | 'org' }; path: string };
};
export type PublishedFile =
  | { kind: 'managed'; target: string; address: FileAddress }
  | { kind: 'local'; root: string; path: string };
export type PublishedFiles = Record<string, PublishedFile>;
export type FileAction = 'inspect' | 'download';
export type RunFileAction = {
  runId: string;
  requestId?: string;
  name: string;
  file: PublishedFile;
  action: FileAction;
};
export type RunFileAccess = {
  /** An authorized inbox read, independent of ordinary status/history access. */
  attachments?: (runId: string, requestId: string, signal: AbortSignal) => Promise<PublishedFiles>;
  actions?: readonly FileAction[];
  open?: (selection: RunFileAction) => void | Promise<void>;
};
export type WorkspaceFilesCapability = {
  kind: 'openengine.workspace-files/v1';
  requestAttachments?: boolean;
  inspect?: boolean;
  download?: boolean;
};

const record = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === 'object' && !Array.isArray(value);
export function isSeedSelection(value: unknown): value is SeedSelection {
  return (
    record(value) &&
    typeof value.member === 'string' &&
    record(value.address) &&
    typeof value.address.path === 'string' &&
    record(value.address.scope) &&
    (value.address.scope.kind === 'user' || value.address.scope.kind === 'org')
  );
}
export function readPublishedFiles(value: unknown): PublishedFiles {
  if (!record(value) || !Object.values(value).every(isPublishedFile))
    throw new Error('File metadata is unavailable.');
  return value as PublishedFiles;
}
function isPublishedFile(value: unknown): value is PublishedFile {
  if (!record(value)) return false;
  if (value.kind === 'local')
    return typeof value.root === 'string' && typeof value.path === 'string';
  return (
    value.kind === 'managed' &&
    typeof value.target === 'string' &&
    record(value.address) &&
    typeof value.address.path === 'string' &&
    record(value.address.scope) &&
    value.address.scope.kind === 'run' &&
    typeof value.address.scope.runId === 'string'
  );
}
