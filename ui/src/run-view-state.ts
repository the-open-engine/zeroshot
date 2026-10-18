import type { HistoryEvent, RunDetail } from './run-history';
import { useEffect, useMemo, useState } from 'react';
import { findNode, type GraphNode } from './domain';
import type { Viewport } from '@xyflow/react';

export type CanvasView = { collapsed: string[]; viewport?: Viewport };
/** Preferences only: admitted definitions and history are always read from the current source. */
export type RunViewState = {
  selected: string;
  inspector: boolean;
  execution: string;
  cursor: string;
  following: boolean;
  canvas?: CanvasView;
};
const token = (value: unknown): value is string => typeof value === 'string' && value.length <= 256;
function validCanvas(canvas: CanvasView) {
  const { collapsed, viewport } = canvas;
  if (!Array.isArray(collapsed) || !collapsed.every(token)) return false;
  return (
    !viewport ||
    ([viewport.x, viewport.y, viewport.zoom].every(Number.isFinite) &&
      viewport.zoom >= 0.15 &&
      viewport.zoom <= 1.8)
  );
}
function validView(value: RunViewState) {
  return (
    value &&
    token(value.selected) &&
    token(value.execution) &&
    typeof value.inspector === 'boolean' &&
    typeof value.following === 'boolean' &&
    typeof value.cursor === 'string' &&
    /^v2:[0-9]+$/.test(value.cursor)
  );
}
export function readRunView(key?: string): RunViewState | undefined {
  if (!key) return;
  try {
    const text = window.sessionStorage.getItem(key);
    if (!text || text.length > 64 * 1024) return;
    const value = JSON.parse(text);
    if (!validView(value)) return;
    if (value.canvas && !validCanvas(value.canvas)) return;
    return value;
  } catch {
    // Optional tab preferences must never prevent opening authoritative history.
  }
}
export function writeRunView(key: string | undefined, value: RunViewState) {
  if (!key) return;
  try {
    const text = JSON.stringify(value);
    if (text.length <= 64 * 1024) window.sessionStorage.setItem(key, text);
  } catch {
    // Storage may be unavailable or full; the current viewer remains usable.
  }
}

export function restoredRunNode(root: GraphNode, saved?: Partial<RunViewState>) {
  const node = saved?.selected ? findNode(root, saved.selected) : undefined;
  return node?.name ?? root.name;
}
export function useRunViewPersistence(
  key: string | undefined,
  run: RunDetail | undefined,
  loading: boolean,
  view: RunViewState
) {
  useEffect(() => {
    if (run && !loading) writeRunView(key, view);
  }, [
    key,
    run,
    loading,
    view.selected,
    view.inspector,
    view.execution,
    view.cursor,
    view.following,
    view.canvas,
  ]);
}

/** Undefined keeps the saved target pending until the next explicitly requested batch. */
export function restoredHistoryPosition(
  cursor: string | undefined,
  events: HistoryEvent[],
  complete: boolean
) {
  const restored = cursor === 'v2:0' ? -1 : events.findIndex((event) => event.cursor === cursor);
  const found = restored >= 0 || cursor === 'v2:0';
  if (!cursor || found || complete) return cursor && found ? restored : events.length - 1;
}

export function historyViewCursor(
  pending: string | undefined,
  events: HistoryEvent[],
  index: number
) {
  return pending ?? events[index]?.cursor ?? 'v2:0';
}

export function useRunView(key?: string) {
  const saved: Partial<RunViewState> = useMemo(() => readRunView(key) ?? {}, [key]);
  const [canvas, setCanvas] = useState<CanvasView | undefined>(saved.canvas);
  return { saved, canvas, setCanvas };
}
