import type { WorkspaceIdentity } from './api';

/** Browser recovery belongs to one authority, mount, and underlying storage workspace. */
export function workspaceStorageKeys(mount: URL, workspace: WorkspaceIdentity) {
  const prefix = `zeroshot.workspace.v1.${encodeURIComponent(
    JSON.stringify([mount.origin, new URL('.', mount).pathname, workspace.kind, workspace.id])
  )}`;
  return {
    draft: `${prefix}.draft`,
    runView: (source: string, runId: string) =>
      `${prefix}.run-view.${encodeURIComponent(JSON.stringify([source, runId]))}`,
    environmentDraft: `${prefix}.environment-draft`,
    lastProfile: `${prefix}.last-profile`,
    layout: (name: string) => `${prefix}.layout.${encodeURIComponent(name)}`,
    // Model IDs only make sense for the harness and provider they were saved with.
    recentModels: (harness: string, provider: string) =>
      `${prefix}.recent-models.${encodeURIComponent(JSON.stringify([harness, provider]))}`,
  };
}
