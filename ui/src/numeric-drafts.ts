import { createContext, useContext } from 'react';
import { allNodes, executable, type GraphNode } from './domain';
import { setPendingEdit } from './pending-edits';
import { numericValue } from './numeric-edit';

export type NumericField = 'timeoutMs' | 'maxIterations' | 'maxItems';
type FieldDraft = { node: string; field: NumericField; text: string; base?: number | null };
let nextScope = 0;

/** Unfinished numbers belong to the document, not an inspector's mounting lifetime. */
export function createNumericDrafts() {
  const scope = `numeric-draft:${++nextScope}:`;
  const values = new Map<string, FieldDraft>();
  const listeners = new Set<() => void>();
  let version = 0;
  const key = (node: string, field: NumericField) => JSON.stringify([node, field]);
  function notify() {
    version++;
    listeners.forEach((listener) => listener());
  }
  function clear(node?: string, field?: NumericField) {
    let changed = false;
    for (const [id, value] of values) {
      if (node !== undefined && value.node !== node) continue;
      if (field !== undefined && value.field !== field) continue;
      values.delete(id);
      setPendingEdit(scope + id, false);
      changed = true;
    }
    if (changed) notify();
  }
  return {
    get: (node: string, field: NumericField) => values.get(key(node, field)),
    set(value: FieldDraft) {
      const id = key(value.node, value.field);
      if (value.text === (value.base == null ? '' : String(value.base))) {
        clear(value.node, value.field);
        return;
      }
      values.set(id, value);
      setPendingEdit(scope + id, true);
      notify();
    },
    commit(
      node: string,
      field: NumericField,
      optional: boolean,
      save: (value: number | undefined) => void
    ) {
      const draft = values.get(key(node, field));
      if (!draft) return false;
      const value = numericValue({ text: draft.text, badInput: false }, optional);
      if (!value.valid) return false;
      save(value.value);
      clear(node, field);
      return true;
    },
    clear,
    rename(before: string, after: string) {
      for (const draft of [...values.values()]) {
        if (draft.node !== before) continue;
        clear(before, draft.field);
        const renamed = { ...draft, node: after };
        const id = key(after, draft.field);
        values.set(id, renamed);
        setPendingEdit(scope + id, true);
      }
      notify();
    },
    prune(root: GraphNode) {
      const nodes = new Map(allNodes(root).map((node) => [node.name, node]));
      for (const draft of [...values.values()]) {
        const node = nodes.get(draft.node);
        const visible =
          node &&
          (draft.field === 'timeoutMs'
            ? executable(node)
            : node.kind === (draft.field === 'maxIterations' ? 'loop' : 'map'));
        if (!visible || (node[draft.field] ?? undefined) !== (draft.base ?? undefined))
          clear(draft.node, draft.field);
      }
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    version: () => version,
  };
}

const context = createContext<ReturnType<typeof createNumericDrafts> | undefined>(undefined);
export const NumericDraftProvider = context.Provider;
export function useNumericDrafts() {
  const value = useContext(context);
  if (!value) throw new Error('Numeric fields require a document draft.');
  return value;
}
