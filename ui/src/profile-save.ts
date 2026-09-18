import type { Saved } from './api';
import { clone, type Document } from './domain';
import type { SaveProfile } from './workspace-services';

const content = (document: Document): Document => ({
  name: document.name,
  graph: document.graph,
  runtime: document.runtime,
});
export type ProfileSaveSnapshot = {
  generation: number;
  original: Document;
  request: SaveProfile;
};

export function snapshotProfileSave(
  document: Document,
  options: { name: string; revision?: string; generation: number; pending: boolean }
): ProfileSaveSnapshot {
  if (options.pending) throw new Error('Finish or cancel the pending field edits before saving.');
  if (!options.name) throw new Error('Enter a profile name.');
  return {
    generation: options.generation,
    original: clone(content(document)),
    request: {
      name: options.name,
      graph: clone(document.graph),
      runtime: clone(document.runtime),
      expectedRevision: options.revision ?? null,
    },
  };
}

/** A successful write acknowledges its snapshot, never a replacement document or later edits. */
export function acknowledgeProfileSave(
  snapshot: ProfileSaveSnapshot,
  current: { generation: number; document: Document },
  result: Saved
): { document: Document; base: Document; revision: string; changed: boolean } | undefined {
  if (snapshot.generation !== current.generation) return;
  if (result.profile.name !== snapshot.request.name || !result.revision)
    throw new Error('The server acknowledged a different profile. Reload before saving again.');
  const changed = JSON.stringify(content(current.document)) !== JSON.stringify(snapshot.original);
  return {
    document: changed
      ? { ...current.document, name: result.profile.name }
      : content(result.profile),
    base: content(result.profile),
    revision: result.revision,
    changed,
  };
}
