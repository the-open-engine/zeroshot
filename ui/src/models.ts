import type { Runtime } from './domain';

// Model IDs are opaque. Suggestions only repeat IDs the user already chose, in this document or in
// profiles they saved earlier; they are never a catalog, an inference, or an availability check.
export type ModelGroup = { label: string; models: string[] };

const RECENT_LIMIT = 10;

/** Distinct non-empty model IDs bound in `runtime`, in binding order. */
export function documentModels(runtime: Runtime): string[] {
  const models = new Set<string>();
  for (const binding of Object.values(runtime.nodes ?? {})) {
    const model = binding?.model;
    if (typeof model === 'string' && model.trim()) models.add(model);
  }
  return [...models];
}

/** Suggestions for a field showing `value`: models bound in the document, then recent choices. */
export function modelGroups(runtime: Runtime, recent: string[], value: string): ModelGroup[] {
  const used = documentModels(runtime);
  return [
    { label: 'In this profile', models: used.filter((model) => model !== value) },
    {
      label: 'Recently used',
      models: recent.filter((model) => model !== value && !used.includes(model)),
    },
  ].filter((group) => group.models.length);
}

export function recentModels(key: string): string[] {
  try {
    const saved = JSON.parse(localStorage.getItem(key) ?? '[]');
    return Array.isArray(saved) ? saved.filter((v) => typeof v === 'string' && v.trim()) : [];
  } catch {
    return [];
  }
}

/** Moves `models` to the front of the recent list stored under `key`. */
export function rememberModels(key: string, models: string[]) {
  if (!models.length) return;
  const next = [...new Set([...models, ...recentModels(key)])].slice(0, RECENT_LIMIT);
  try {
    localStorage.setItem(key, JSON.stringify(next));
  } catch {
    // Suggestions are a convenience; a full or blocked storage only loses them.
  }
}
