import type { WorkspaceIdentity } from './api';

/** Browser recovery belongs to one authority, mount, and underlying storage workspace. */
export function workspaceStorageKeys(mount: URL, workspace: WorkspaceIdentity) {
  const prefix = `zeroshot.workspace.v1.${encodeURIComponent(
    JSON.stringify([mount.origin, new URL('.', mount).pathname, workspace.kind, workspace.id])
  )}`;
  return {
    draft: `${prefix}.draft`,
    lastProfile: `${prefix}.last-profile`,
    layout: (name: string) => `${prefix}.layout.${encodeURIComponent(name)}`,
  };
}
