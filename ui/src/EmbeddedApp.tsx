import { useEffect, useState } from 'react';
import { RunHistoryView } from './RunHistoryView';
import { RunHistoryBoundary } from './RunHistoryBoundary';
import { createEmbeddedFetch } from './embedded-fetch';
import { App } from './App';
import { EnvironmentWorkspace } from './EnvironmentWorkspace';
import { createEnvironmentStore, type EnvironmentStore } from './environment-store';
import { WorkspaceBridge, type WorkspaceInit } from './workspace-bridge';
import { createWorkspaceServices, type WorkspaceServices } from './workspace-services';
import { ApiError, type Bootstrap } from './api';

export function EmbeddedApp() {
  const [host, setHost] = useState<WorkspaceBridge>();
  const [configuration, setConfiguration] = useState<WorkspaceInit>();
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
  const { loaded, error: loadError } = useHostedServices(configuration, host);
  const message = error || loadError;
  if (!loaded || !host)
    return (
      <main className="fatal" role="status">
        <h1>{message ? 'Could not open workspace' : 'Opening workspace'}</h1>
        {message && <p role="alert">{message}</p>}
      </main>
    );
  if (configuration?.view === 'environments' && loaded.collection && loaded.environmentStore)
    return (
      <EnvironmentWorkspace
        store={loaded.environmentStore}
        collection={loaded.collection}
        bootstrap={loaded.bootstrap}
        host={host}
        readOnly={configuration.readOnly}
      />
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

function useHostedServices(configuration?: WorkspaceInit, host?: WorkspaceBridge) {
  const [loaded, setLoaded] = useState<{
    services: WorkspaceServices;
    bootstrap: Bootstrap;
    environmentStore?: EnvironmentStore;
    collection?: URL;
  }>();
  const [error, setError] = useState('');
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
        const collection = configuration.environmentApi
          ? new URL(configuration.environmentApi, window.location.origin)
          : undefined;
        const environmentStore = collection
          ? createEnvironmentStore(
              collection,
              bootstrap.workspace.id,
              createEmbeddedFetch(base, configuration.csrf)
            )
          : undefined;
        setLoaded({ services, bootstrap, collection, environmentStore });
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
  return { loaded, error };
}
