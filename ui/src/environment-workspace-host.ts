import { useEffect, useRef, useState, type MutableRefObject } from 'react';
import { ApiError } from './api';
import { hasPendingEdits } from './pending-edits';
import type { Environment, EnvironmentSaveSnapshot } from './environment-store';
import type { WorkspaceBridge, HostCommand } from './workspace-bridge';

type State = {
  dirty: boolean;
  pending: boolean;
  busy: boolean;
  loading: boolean;
  hasDraft: boolean;
  name?: string;
  resourceId: string | null;
  resourceRevision: string | null;
};
type Actions = {
  discard: () => void;
  capture: () => Promise<EnvironmentSaveSnapshot>;
  acknowledge: (captured: EnvironmentSaveSnapshot, saved: Environment) => void;
};
type PendingSave = { requestId: string; snapshot?: EnvironmentSaveSnapshot };
type Latest = { state: State; actions: Actions; readOnly: boolean };
type Active = { current: boolean };
type HostContext = {
  host: WorkspaceBridge;
  documentId: string;
  pending: MutableRefObject<PendingSave | undefined>;
  latest: MutableRefObject<Latest>;
  settingsDirty: MutableRefObject<boolean>;
  active: Active;
  setSaving: (value: boolean) => void;
  setHostDirty: (value: boolean) => void;
};
type Problem = { code: string; message: string; details?: unknown };

/** The host owns persistence; the iframe captures and acknowledges one conditional snapshot. */
export function useEnvironmentHost(
  host: WorkspaceBridge | undefined,
  state: State,
  actions: Actions,
  settingsDirty: MutableRefObject<boolean>,
  readOnly: boolean
) {
  const [documentId] = useState(() => crypto.randomUUID());
  const [saving, setSaving] = useState(false);
  const [hostDirty, setHostDirty] = useState(false);
  const pending = useRef<PendingSave | undefined>(undefined);
  const latest = useRef({ state, actions, readOnly });
  latest.current = { state, actions, readOnly };
  useEffect(() => {
    if (!host) return;
    const context: HostContext = {
      host,
      documentId,
      pending,
      latest,
      settingsDirty,
      active: { current: true },
      setSaving,
      setHostDirty,
    };
    const disconnect = host.connect((command) => void receiveHostCommand(command, context));
    return () => {
      context.active.current = false;
      disconnect();
    };
  }, [host, documentId, settingsDirty]);
  useEffect(() => {
    host?.send(
      'state',
      {
        dirty: state.dirty || hostDirty,
        pending: state.pending,
        saving: state.busy || saving,
        loading: state.loading,
        hasDraft: state.hasDraft,
        name: state.name,
        resourceId: state.resourceId,
        resourceRevision: state.resourceRevision,
      },
      { documentId, generation: 0 }
    );
  }, [
    host,
    state.dirty,
    state.pending,
    state.busy,
    state.loading,
    state.hasDraft,
    state.name,
    state.resourceId,
    state.resourceRevision,
    hostDirty,
    saving,
    documentId,
  ]);
  useEffect(() => {
    const unload = (event: BeforeUnloadEvent) => {
      if (
        latest.current.state.dirty ||
        settingsDirty.current ||
        saving ||
        latest.current.state.busy ||
        hasPendingEdits()
      ) {
        event.preventDefault();
        event.returnValue = '';
      }
    };
    window.addEventListener('beforeunload', unload);
    return () => window.removeEventListener('beforeunload', unload);
  }, [saving, settingsDirty]);
  return saving;
}

async function receiveHostCommand(command: HostCommand, context: HostContext) {
  try {
    requireDocument(command, context.documentId);
    if (command.type === 'request_save') {
      await captureSnapshot(command, context);
      return;
    }
    if (command.type === 'save_ack') {
      acknowledgeSnapshot(command, context);
      return;
    }
    acceptHostCommand(command, context);
    reply(command, context);
  } catch (error) {
    reply(command, context, asProblem(error));
  }
}

function acceptHostCommand(command: HostCommand, context: HostContext) {
  if (command.type === 'theme') {
    document.documentElement.dataset.theme = command.theme;
    return;
  }
  if (command.type === 'environment_settings') {
    context.settingsDirty.current = command.dirty;
    if (context.active.current) context.setHostDirty(command.dirty);
    return;
  }
  if (command.type !== 'navigate')
    throw new ApiError(
      400,
      'unsupported_command',
      'This action is unavailable in the environment workspace.'
    );
  protectNavigation(
    command,
    context.latest.current.state,
    context.pending.current !== undefined,
    context.settingsDirty.current
  );
  if (!command.discard) return;
  context.settingsDirty.current = false;
  if (context.active.current) context.setHostDirty(false);
  context.latest.current.actions.discard();
}

async function captureSnapshot(
  command: HostCommand & { type: 'request_save' },
  context: HostContext
) {
  const current = context.latest.current;
  if (current.readOnly)
    throw new ApiError(403, 'environment_read_only', 'This environment is read-only.');
  if (context.pending.current || current.state.busy || current.state.loading)
    throw new ApiError(409, 'save_pending', 'Wait for the current environment operation.');
  context.pending.current = { requestId: command.requestId };
  if (context.active.current) context.setSaving(true);
  try {
    const snapshot = await current.actions.capture();
    if (!context.active.current) return;
    context.pending.current = { requestId: command.requestId, snapshot };
    context.host.send(
      'save_snapshot',
      { environment: snapshot, authority: context.host.configuration?.authority },
      { documentId: context.documentId, generation: 0, requestId: command.requestId }
    );
  } catch (error) {
    context.pending.current = undefined;
    if (context.active.current) context.setSaving(false);
    throw error;
  }
}

function acknowledgeSnapshot(command: HostCommand & { type: 'save_ack' }, context: HostContext) {
  const captured = context.pending.current;
  if (!captured?.snapshot || captured.requestId !== command.requestId)
    throw new ApiError(
      409,
      'unknown_snapshot',
      'This save acknowledgement has no pending snapshot.'
    );
  context.pending.current = undefined;
  if (context.active.current) context.setSaving(false);
  if (command.problem) {
    reply(command, context, command.problem);
    return;
  }
  if (!command.environment)
    throw new ApiError(502, 'invalid_environment_response', 'The save could not be confirmed.');
  context.settingsDirty.current = false;
  if (context.active.current) context.setHostDirty(false);
  context.latest.current.actions.acknowledge(captured.snapshot, command.environment);
  reply(command, context);
}

function reply(command: HostCommand, context: HostContext, problem?: Problem) {
  context.host.send(
    'state',
    { accepted: !problem, problem },
    { documentId: context.documentId, generation: 0, requestId: command.requestId }
  );
}

function requireDocument(command: HostCommand, documentId: string) {
  if (command.documentId !== documentId || command.generation !== 0)
    throw new ApiError(409, 'document_changed', 'The environment workspace changed. Reopen it.');
}

function protectNavigation(
  command: HostCommand & { type: 'navigate' },
  state: State,
  saving: boolean,
  settingsDirty: boolean
) {
  if (saving || state.busy || state.loading)
    throw new ApiError(409, 'workspace_busy', 'Wait for the current environment operation.');
  if ((state.dirty || settingsDirty || state.pending || hasPendingEdits()) && !command.discard)
    throw new ApiError(409, 'unsaved_changes', 'Save or discard the environment before leaving.');
}

function asProblem(error: unknown) {
  return {
    code: error instanceof ApiError ? error.code : 'workspace_error',
    message: error instanceof Error ? error.message : String(error),
    ...(error instanceof ApiError ? { details: error.details } : {}),
  };
}
