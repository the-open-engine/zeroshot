import { useEffect, useRef, useState } from 'react';
import { WorkspaceSession, type EditorState } from './workspace-session';
import type { WorkspaceBridge } from './workspace-bridge';

export function useHostWorkspace(host: WorkspaceBridge | undefined, editor: EditorState) {
  const latest = useRef(editor);
  latest.current = editor;
  const [, changed] = useState(0);
  const [session] = useState(
    () =>
      new WorkspaceSession(
        host,
        () => latest.current,
        () => changed((n) => n + 1)
      )
  );
  useEffect(
    () =>
      host?.connect((command) => {
        void session.receive(command);
      }),
    [host, session]
  );
  useEffect(() => {
    session.state();
  }, [host, editor, session.documentId, session.saving]);
  return {
    documentId: session.documentId,
    profileDocumentId: session.profileDocumentId,
    runId: session.runId,
    showingRun: session.showingRun,
    restore: (id?: string) => session.restore(id),
    requestSave: () => session.navigate('save'),
    navigate: (action: 'defaults') => session.navigate(action),
  };
}
