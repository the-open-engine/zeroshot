import { useRef, useState } from 'react';
import { useEnvironmentDraft } from './environment-draft';
import { useEnvironmentCatalog } from './use-environment-catalog';
import { useEnvironmentOperation } from './use-environment-operation';
import { flushPendingEdits, hasPendingEdits, usePendingEdits } from './pending-edits';
import type { Environment, EnvironmentDraft, EnvironmentStore } from './environment-store';

export function useEnvironmentWorkspace(
  store: EnvironmentStore,
  readOnly: boolean,
  draftKey: string
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
  const latest = useRef(draft);
  latest.current = draft;
  function leave() {
    return !(dirty || hasPendingEdits()) || window.confirm('Discard unsaved environment changes?');
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
    save: () =>
      void perform(async (signal) => {
        if (readOnly) return;
        await flushPendingEdits();
        const value = latest.current;
        if (!value) return;
        const result = await store.save(
          { ...value, expectedRevision: saved?.revision ?? null },
          signal
        );
        if (signal.aborted) return;
        adopt(result);
        setItems((previous) =>
          [...previous.filter((item) => item.id !== result.id), result].sort((a, b) =>
            a.name.localeCompare(b.name)
          )
        );
        setNotice('Environment saved. New runs use this version.');
      }),
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
