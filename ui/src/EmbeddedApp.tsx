import { useEffect, useState } from 'react';
import { RunHistoryView } from './RunHistoryView';
import { RunHistoryBoundary } from './RunHistoryBoundary';
import { createEmbeddedFetch } from './embedded-fetch';
import { App } from './App';
import { WorkspaceBridge, type WorkspaceInit } from './workspace-bridge';
import { createWorkspaceServices, type WorkspaceServices } from './workspace-services';
import { ApiError, type Bootstrap } from './api';

export function EmbeddedApp() {
  const [host, setHost] = useState<WorkspaceBridge>();
  const [configuration, setConfiguration] = useState<WorkspaceInit>();
  const [loaded, setLoaded] = useState<{ services: WorkspaceServices; bootstrap: Bootstrap }>();
  const [error, setError] = useState('');
  useEffect(() => {
    if (window.parent === window) {
      setError('Open this workspace from its host.');
      return;
    }
    const bridge = new WorkspaceBridge(
      window.location.origin,
      window.parent,
      (listener) => {
        window.addEventListener('message', listener);
        return () => window.removeEventListener('message', listener);
      },
      setConfiguration
    );
    setHost(bridge);
    return () => bridge.dispose();
  }, []);
  useEffect(() => {
    if (!configuration || !host) return;
    document.documentElement.dataset.theme = configuration.theme;
    const controller = new AbortController();
    const base = new URL(configuration.apiBase, window.location.origin);
    const services = createWorkspaceServices(
      new URL('.', document.baseURI),
      createEmbeddedFetch(base, configuration.csrf),
      base
    );
    void services
      .catalog(controller.signal)
      .then((bootstrap) => {
        if (controller.signal.aborted) return;
        if (
          bootstrap.workspace.kind !== 'cloud' ||
          bootstrap.workspace.id !== configuration.workspaceId
        )
          throw new ApiError(
            409,
            'workspace_changed',
            'Workspace authority changed. Reopen it from the host.'
          );
        setLoaded({ services, bootstrap });
      })
      .catch((cause) => {
        if (controller.signal.aborted) return;
        const message = cause instanceof Error ? cause.message : String(cause);
        setError(message);
        host.send(
          'state',
          {
            problem: {
              code: cause instanceof ApiError ? cause.code : 'workspace_unavailable',
              message,
            },
          },
          { documentId: null, generation: 0, requestId: configuration.requestId }
        );
      });
    return () => controller.abort();
  }, [configuration, host]);
  if (!loaded || !host)
    return (
      <main className="fatal" role="status">
        <h1>{error ? 'Could not open workspace' : 'Opening workspace'}</h1>
        {error && <p role="alert">{error}</p>}
      </main>
    );
  return (
    <App
      services={loaded.services}
      bootstrap={loaded.bootstrap}
      host={host}
      renderRun={(runId) => (
        <RunHistoryBoundary workspace={loaded.bootstrap.workspace} embedded>
          <RunHistoryView
            runId={runId}
            source={loaded.services.history}
            workers={loaded.bootstrap.workers ?? []}
          />
        </RunHistoryBoundary>
      )}
    />
  );
}
