import { useRef, useState } from 'react';
import { ApiError } from './api';
import { useEnvironmentDraft } from './environment-draft';
import { useEnvironmentCatalog } from './use-environment-catalog';
import { useEnvironmentOperation } from './use-environment-operation';
import { flushPendingEdits, hasPendingEdits, usePendingEdits } from './pending-edits';
import type {
  Environment,
  EnvironmentDraft,
  EnvironmentSaveSnapshot,
  EnvironmentStore,
} from './environment-store';

export function useEnvironmentWorkspace(
  store: EnvironmentStore,
  readOnly: boolean,
  draftKey: string,
  settingsDirty: () => boolean
) {
  const catalog = useEnvironmentCatalog(store);
  const { items, setItems, loading } = catalog;
  const recovery = useEnvironmentDraft(draftKey);
  const { draft, saved, dirty } = recovery;
  // Adopting a document also discards field-local text, even when saved values are unchanged.
  const [editorGeneration, setEditorGeneration] = useState(0);
  function adopt(value?: Environment) {
    recovery.adopt(value);
    setEditorGeneration((generation) => generation + 1);
  }
  function discard() {
    recovery.discard();
    setEditorGeneration((generation) => generation + 1);
  }
  const operation = useEnvironmentOperation(store, loading);
  const { busy, error, notice, setError, setNotice, perform } = operation;
  const pending = usePendingEdits();
  const latest = useRef({ draft, saved });
  latest.current = { draft, saved };
  function leave() {
    return (
      !(dirty || settingsDirty() || hasPendingEdits()) ||
      window.confirm('Discard unsaved environment changes?')
    );
  }
  return {
    items,
    editorGeneration,
    draft,
    saved,
    busy,
    loading,
    dirty,
    pending,
    error: error || catalog.error,
    notice: notice || recovery.warning,
    discard,
    change: (value: EnvironmentDraft) => {
      if (!readOnly) recovery.change(value);
    },
    create: () => {
      if (readOnly || busy || loading || !leave()) return;
      adopt();
      recovery.change({ name: '', definition: {} });
      setError('');
      setNotice('');
    },
    open: (id: string) =>
      void perform(async (signal) => {
        if (!leave()) return;
        const value = await store.load(id, signal);
        if (!signal.aborted) adopt(value);
      }),
    capture: () => captureEnvironment(() => latest.current, readOnly),
    acknowledge: (captured: EnvironmentSaveSnapshot, result: Environment) => {
      recovery.acknowledge(captured, result);
      setItems((previous) =>
        [...previous.filter((item) => item.id !== result.id), result].sort((a, b) =>
          a.name.localeCompare(b.name)
        )
      );
      setError('');
      setNotice('');
    },
    remove: () =>
      void perform(async (signal) => {
        if (readOnly || !saved || !leave() || !window.confirm(`Delete ${saved.name}?`)) return;
        await store.remove(saved.id, saved.revision, signal);
        if (signal.aborted) return;
        setItems((previous) => previous.filter((item) => item.id !== saved.id));
        adopt();
        setNotice('Environment deleted. Existing runs keep their saved environment.');
      }),
  };
}

async function captureEnvironment(
  current: () => { draft?: EnvironmentDraft; saved?: Environment },
  readOnly: boolean
): Promise<EnvironmentSaveSnapshot> {
  if (readOnly)
    throw new ApiError(403, 'environment_read_only', 'This environment is read-only.');
  try {
    await flushPendingEdits();
  } catch (error) {
    throw new ApiError(
      409,
      'unresolved_edits',
      error instanceof Error ? error.message : 'Finish the pending field edits.'
    );
  }
  const value = current();
  if (!value.draft)
    throw new ApiError(409, 'environment_required', 'Select an environment before saving.');
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(value.draft.name))
    throw new ApiError(
      400,
      'invalid_environment_name',
      'Use 1–64 letters, numbers, dots, underscores, or hyphens.'
    );
  return structuredClone({
    ...value.draft,
    expectedRevision: value.saved?.revision ?? null,
  });
}
