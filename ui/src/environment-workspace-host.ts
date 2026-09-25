import { useEffect, useRef, useState } from 'react';
import { ApiError } from './api';
import { hasPendingEdits } from './pending-edits';
import type { WorkspaceBridge, HostCommand } from './workspace-bridge';

type State = { dirty: boolean; pending: boolean; busy: boolean; loading: boolean; name?: string };
/** The host owns navigation and authority; environment persistence uses the generic service. */
export function useEnvironmentHost(
  host: WorkspaceBridge | undefined,
  state: State,
  discard: () => void
) {
  const [documentId] = useState(() => crypto.randomUUID());
  const latest = useRef({ state, discard });
  latest.current = { state, discard };
  useEffect(
    () =>
      host?.connect((command) => {
        let problem: { code: string; message: string } | undefined;
        try {
          accept(command, latest.current.state, documentId);
          if (command.type === 'navigate' && command.discard) latest.current.discard();
        } catch (error) {
          problem = {
            code: error instanceof ApiError ? error.code : 'workspace_error',
            message: error instanceof Error ? error.message : String(error),
          };
        }
        host.send(
          'state',
          { accepted: !problem, problem },
          { documentId, generation: 0, requestId: command.requestId }
        );
      }),
    [host, documentId]
  );
  useEffect(() => {
    host?.send(
      'state',
      {
        dirty: state.dirty,
        pending: state.pending,
        saving: state.busy,
        loading: state.loading,
        name: state.name,
      },
      { documentId, generation: 0 }
    );
  }, [host, state.dirty, state.pending, state.busy, state.loading, state.name, documentId]);
  useEffect(() => {
    const unload = (event: BeforeUnloadEvent) => {
      if (latest.current.state.dirty || latest.current.state.busy || hasPendingEdits()) {
        event.preventDefault();
        event.returnValue = '';
      }
    };
    window.addEventListener('beforeunload', unload);
    return () => window.removeEventListener('beforeunload', unload);
  }, []);
}
function accept(command: HostCommand, state: State, documentId: string) {
  if (command.documentId !== documentId || command.generation !== 0)
    throw new ApiError(409, 'document_changed', 'The environment workspace changed. Reopen it.');
  if (command.type === 'theme') {
    document.documentElement.dataset.theme = command.theme;
    return;
  }
  if (command.type !== 'navigate')
    throw new ApiError(
      400,
      'unsupported_command',
      'This action is unavailable in the environment workspace.'
    );
  protectNavigation(command, state);
}
function protectNavigation(command: HostCommand & { type: 'navigate' }, state: State) {
  if (state.busy || state.loading)
    throw new ApiError(409, 'workspace_busy', 'Wait for the current environment operation.');
  if ((state.dirty || state.pending || hasPendingEdits()) && !command.discard)
    throw new ApiError(409, 'unsaved_changes', 'Save or discard the environment before leaving.');
}
