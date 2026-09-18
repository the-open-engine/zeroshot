import React, { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import '@fontsource-variable/fraunces';
import '@fontsource-variable/spline-sans';
import '@xyflow/react/dist/style.css';
import './styles.css';
import { initializeTheme } from './theme';
import { App } from './App';
import { createWorkspaceServices } from './workspace-services';
import { workspaceStorageKeys } from './workspace-storage';
import type { Bootstrap } from './api';
const services = createWorkspaceServices(new URL('.', document.baseURI));
class ErrorBoundary extends React.Component<
  { children: React.ReactNode; draftKey: string },
  { error: boolean }
> {
  state = { error: false };
  static getDerivedStateFromError() {
    return { error: true };
  }
  render() {
    return this.state.error ? (
      <main className="fatal">
        <h1>Could not display this profile</h1>
        <p>Your saved profiles have not changed.</p>
        <button
          onClick={() => {
            try {
              sessionStorage.removeItem(this.props.draftKey);
            } catch {
              /* optional recovery */
            }
            location.reload();
          }}
        >
          Reopen saved profiles
        </button>
      </main>
    ) : (
      this.props.children
    );
  }
}
function StandaloneApp() {
  const [bootstrap, setBootstrap] = useState<Bootstrap>();
  const [error, setError] = useState('');
  useEffect(() => {
    const controller = new AbortController();
    void services
      .catalog(controller.signal)
      .then((value) => {
        if (!controller.signal.aborted) setBootstrap(value);
      })
      .catch((cause) => {
        if (!controller.signal.aborted)
          setError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => controller.abort();
  }, []);
  if (!bootstrap)
    return (
      <main className="fatal">
        <h1>{error ? 'Could not open Zeroshot' : 'Opening Zeroshot'}</h1>
        {error && (
          <>
            <p role="alert">{error}</p>
            <button onClick={() => location.reload()}>Retry</button>
          </>
        )}
      </main>
    );
  const storage = workspaceStorageKeys(services.mount, bootstrap.workspace);
  return (
    <ErrorBoundary key={storage.draft} draftKey={storage.draft}>
      <App services={services} bootstrap={bootstrap} />
    </ErrorBoundary>
  );
}
initializeTheme();
createRoot(document.getElementById('root')!).render(<StandaloneApp />);
