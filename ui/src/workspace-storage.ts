import type { WorkspaceIdentity } from './api';

const storagePrefix = 'zeroshot.workspace.v1.';
const versionedWorkspaceMount = /^\/workspace\/[0-9a-f]{40}\/$/;

function workspacePrefix(mount: URL, kind: string, id: string) {
  const path = new URL('.', mount).pathname;
  const storageMount =
    kind === 'platform' && versionedWorkspaceMount.test(path) ? '/workspace/' : path;
  return `${storagePrefix}${encodeURIComponent(
    JSON.stringify([mount.origin, storageMount, kind, id])
  )}`;
}

/** Browser recovery belongs to one authority, mount, and underlying storage workspace. */
export function workspaceStorageKeys(mount: URL, workspace: WorkspaceIdentity) {
  const prefix = workspacePrefix(mount, workspace.kind, workspace.id);
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

/** Recover the previous tag's browser state after the authenticated bootstrap identifies it. */
export function upgradePlatformWorkspaceStorage(
  mount: URL,
  workspace: WorkspaceIdentity,
  previousMount?: string
) {
  if (workspace.kind !== 'platform' || typeof window === 'undefined') return;
  const previous = [workspacePrefix(mount, 'cloud', workspace.id) + '.'];
  if (
    versionedWorkspaceMount.test(mount.pathname) &&
    typeof previousMount === 'string' &&
    versionedWorkspaceMount.test(previousMount)
  )
    previous.push(workspacePrefix(new URL(previousMount, mount), 'cloud', workspace.id) + '.');
  const prefix = workspacePrefix(mount, workspace.kind, workspace.id) + '.';
  for (const name of ['localStorage', 'sessionStorage'] as const) {
    try {
      upgradeStorage(window[name], prefix, previous);
    } catch {
      // Storage can be blocked or full. Keep unhandled old values available for a later retry.
    }
  }
}

function upgradeStorage(storage: Storage, prefix: string, previous: string[]) {
  const marker = prefix + 'upgraded-from-cloud';
  const saved = storage.getItem(marker);
  if (saved === '1') return;
  const entries = saved === null ? legacyEntries(storage, previous) : readUpgrade(saved);
  if (!entries) return;
  // Reserve the complete journal first. Updating 0 to 1 cannot require more quota space.
  if (saved === null) storage.setItem(marker, JSON.stringify(entries));
  for (const [suffix, completed] of Object.entries(entries)) {
    if (completed === 1) continue;
    const oldKeys = previous.map((oldPrefix) => oldPrefix + suffix);
    upgradeEntry(storage, oldKeys, prefix + suffix);
    entries[suffix] = 1;
    storage.setItem(marker, JSON.stringify(entries));
    for (const oldKey of oldKeys) storage.removeItem(oldKey);
  }
  storage.setItem(marker, '1');
}

function upgradeEntry(storage: Storage, oldKeys: string[], key: string) {
  if (storage.getItem(key) !== null) return;
  for (const oldKey of oldKeys) {
    const value = storage.getItem(oldKey);
    if (value !== null) {
      storage.setItem(key, value);
      return;
    }
  }
}

function legacyEntries(storage: Storage, previous: string[]) {
  const entries: Record<string, 0 | 1> = Object.create(null);
  for (let index = 0; index < storage.length; index++) {
    const key = storage.key(index);
    if (key === null) continue;
    const oldPrefix = previous.find((candidate) => key.startsWith(candidate));
    if (oldPrefix !== undefined) entries[key.slice(oldPrefix.length)] = 0;
  }
  return entries;
}

function readUpgrade(saved: string): Record<string, 0 | 1> | undefined {
  try {
    const value: unknown = JSON.parse(saved);
    if (
      value !== null &&
      typeof value === 'object' &&
      !Array.isArray(value) &&
      Object.values(value).every((completed) => completed === 0 || completed === 1)
    )
      return value as Record<string, 0 | 1>;
  } catch {
    // A damaged completion journal cannot authorize importing potentially discarded old state.
  }
}
