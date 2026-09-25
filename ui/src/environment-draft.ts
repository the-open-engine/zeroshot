import { useEffect, useState } from 'react';
import type { Environment, EnvironmentDefinition, EnvironmentDraft } from './environment-store';

type DraftState = { key: string; draft?: EnvironmentDraft; saved?: Environment };
const MAX_RECOVERY_BYTES = 8 * 1024 * 1024;
const recoveryUnavailable = 'Draft recovery is unavailable. Save your changes before closing.';
/** A saved revision is recovery context, never authority for another workspace. */
export function useEnvironmentDraft(key: string) {
  const [state, setState] = useState(() => restore(key));
  const [warning, setWarning] = useState('');
  const current = state.key === key ? state : { key };
  const dirty =
    !!current.draft &&
    JSON.stringify(current.draft) !== JSON.stringify(current.saved && content(current.saved));
  useEffect(() => {
    setState((previous) => (previous.key === key ? previous : restore(key)));
  }, [key]);
  useEffect(() => {
    if (state.key !== key) return;
    try {
      if (dirty) {
        const snapshot = JSON.stringify({ draft: state.draft, saved: state.saved });
        if (!bounded(snapshot)) {
          sessionStorage.removeItem(key);
          setWarning(recoveryUnavailable);
          return;
        }
        sessionStorage.setItem(key, snapshot);
      } else sessionStorage.removeItem(key);
      setWarning('');
    } catch {
      setWarning(recoveryUnavailable);
    }
  }, [state, key, dirty]);
  function discard() {
    try {
      sessionStorage.removeItem(key);
    } catch {
      setWarning(recoveryUnavailable);
    }
    setState((previous) => ({
      key,
      saved: previous.key === key ? previous.saved : undefined,
      draft: previous.key === key && previous.saved ? content(previous.saved) : undefined,
    }));
  }
  return {
    draft: current.draft,
    saved: current.saved,
    dirty,
    warning,
    discard,
    change: (draft: EnvironmentDraft) =>
      setState((previous) => ({
        ...(previous.key === key ? previous : { key }),
        draft,
      })),
    adopt: (saved?: Environment) => setState({ key, saved, draft: saved && content(saved) }),
  };
}
function content(value: Environment): EnvironmentDraft {
  return { id: value.id, name: value.name, definition: value.definition };
}
function restore(key: string): DraftState {
  try {
    const text = sessionStorage.getItem(key);
    if (!text || !bounded(text)) return { key };
    const value: unknown = JSON.parse(text);
    if (!record(value) || !draft(value.draft)) return { key };
    const saved = value.saved;
    if (saved !== undefined && !savedBase(saved, value.draft.id)) return { key };
    return { key, draft: value.draft, saved: saved as Environment | undefined };
  } catch {
    return { key };
  }
}
function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}
function draft(value: unknown): value is EnvironmentDraft {
  return (
    record(value) &&
    typeof value.name === 'string' &&
    value.name.length <= 64 &&
    (value.id === undefined || typeof value.id === 'string') &&
    definition(value.definition)
  );
}
function definition(value: unknown): value is EnvironmentDefinition {
  if (!record(value)) return false;
  return Object.entries(value).every(([name, field]) => {
    if (name === 'setup' || name === 'startup') return typeof field === 'string';
    if (name === 'variables')
      return record(field) && Object.values(field).every((entry) => typeof entry === 'string');
    if (name === 'connections')
      return (
        record(field) &&
        Object.values(field).every(
          (entry) => Array.isArray(entry) && entry.every((item) => typeof item === 'string')
        )
      );
    return false;
  });
}

function savedBase(value: unknown, id?: string): value is Environment {
  if (!record(value)) return false;
  const revision = value.revision;
  return (
    draft(value) &&
    typeof value.id === 'string' &&
    !!value.id &&
    value.id === id &&
    typeof revision === 'string' &&
    revision.length > 0 &&
    revision.length <= 256
  );
}

function bounded(text: string): boolean {
  return (
    text.length <= MAX_RECOVERY_BYTES &&
    new TextEncoder().encode(text).byteLength <= MAX_RECOVERY_BYTES
  );
}
