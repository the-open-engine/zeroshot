import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import {
  readPublishedFiles,
  type PublishedFiles as Files,
  type RunFileAccess,
  type RunFileAction,
} from './file-workflows';

const FileAccess = createContext<{ runId?: string; access?: RunFileAccess }>({});
export function RunFilesProvider({
  runId,
  access,
  children,
}: {
  runId?: string;
  access?: RunFileAccess;
  children: ReactNode;
}) {
  return <FileAccess.Provider value={{ runId, access }}>{children}</FileAccess.Provider>;
}

/** No URL or filesystem authority is inferred from a path in a graph's ordinary output. */
export function PublishedFiles({
  files,
  requestId,
  runId: selectedRunId,
}: {
  files?: Files;
  requestId?: string;
  runId?: string;
}) {
  const { runId: currentRunId, access } = useContext(FileAccess);
  const runId = selectedRunId ?? currentRunId;
  const [error, setError] = useState('');
  const entries = Object.entries(files ?? {});
  const canOpen = !!(runId && access?.open && access.actions?.length);
  if (!entries.length) return null;
  async function open(selection: RunFileAction) {
    setError('');
    try {
      await access?.open?.(selection);
    } catch {
      setError('The host could not open this file or folder.');
    }
  }
  return (
    <section
      className="published-files"
      aria-label={requestId ? 'Request attachments' : 'Published files'}
    >
      <h3>{requestId ? 'Attachments' : 'Published files'}</h3>
      {entries.map(([name, file]) => (
        <div className="published-file" key={name}>
          <strong>{name}</strong>
          <p className="history-muted">
            {file.kind === 'managed'
              ? 'Retained publication'
              : 'Live local path · current contents'}
          </p>
          <p className="mono">
            {file.kind === 'managed'
              ? file.address.path
              : `${file.root}${file.root.endsWith('/') || file.root.endsWith('\\') ? '' : '/'}${file.path}`}
          </p>
          {file.kind === 'managed' && (
            <p className="history-muted">
              Target {file.target} · part{' '}
              {file.address.scope.kind === 'run' ? file.address.scope.runId : ''}
            </p>
          )}
          {runId &&
            access?.open &&
            access.actions?.map((action) => (
              <button
                type="button"
                className="button compact"
                key={action}
                onClick={() => void open({ runId, requestId, name, file, action })}
              >
                {action === 'inspect' ? 'Inspect' : 'Download'} {name}
              </button>
            ))}
          {!canOpen && (
            <p className="history-muted">
              {file.kind === 'managed'
                ? 'Retrieve with zeroshot file download or Client.download_file().'
                : 'Open this path on the local machine. Its contents may change or be removed.'}
            </p>
          )}
        </div>
      ))}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}

/** Only a host-advertised request-authorized reader can expose attachment metadata. */
export function RequestAttachments({ requestId, runId }: { requestId: string; runId: string }) {
  const { access } = useContext(FileAccess);
  const [loaded, setLoaded] = useState<{
    key: string;
    access: RunFileAccess;
    files?: Files;
    error?: string;
  }>();
  const key = JSON.stringify([runId, requestId]);
  useEffect(() => {
    if (!access?.attachments) return;
    const controller = new AbortController();
    void access
      .attachments(runId, requestId, controller.signal)
      .then((value) => {
        if (!controller.signal.aborted)
          setLoaded({ key, access, files: readPublishedFiles(value) });
      })
      .catch(() => {
        if (!controller.signal.aborted)
          setLoaded({ key, access, error: 'Attachment metadata is unavailable.' });
      });
    return () => controller.abort();
  }, [access, runId, requestId, key]);
  if (!access?.attachments || loaded?.key !== key || loaded.access !== access) return null;
  return loaded.error ? (
    <p className="history-muted" role="status">
      {loaded.error}
    </p>
  ) : (
    <PublishedFiles files={loaded.files} requestId={requestId} runId={runId} />
  );
}
