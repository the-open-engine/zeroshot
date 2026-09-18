import { useId, useLayoutEffect, useSyncExternalStore } from 'react';

const pending = new Set<string>();
const listeners = new Set<() => void>();
export const hasPendingEdits = () => pending.size > 0;
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};
export function setPendingEdit(id: string, dirty: boolean) {
  const before = hasPendingEdits();
  if (dirty) pending.add(id);
  else pending.delete(id);
  if (before !== hasPendingEdits()) listeners.forEach((listener) => listener());
}
// Renames and enum text commit on blur so intermediate invalid names never enter
// GraphSpec. Their in-progress text must still participate in navigation protection.
export function usePendingText(dirty: boolean) {
  const id = useId();
  useLayoutEffect(() => {
    setPendingEdit(id, dirty);
    return () => setPendingEdit(id, false);
  }, [id, dirty]);
}
export const usePendingEdits = () => useSyncExternalStore(subscribe, hasPendingEdits, () => false);

/** Commit the focused field, then refuse to save any text which still has no valid value. */
export async function flushPendingEdits(): Promise<void> {
  (document.activeElement as HTMLElement | null)?.blur();
  await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  if (hasPendingEdits()) throw new Error('Finish or cancel the pending field edits before saving.');
}
