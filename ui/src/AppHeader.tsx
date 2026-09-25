import type { ReactNode } from 'react';
import { ThemeControl } from './ThemeControl';
import type { WorkspaceIdentity } from './api';

export function AppHeader({
  section,
  navigate,
  children,
  workspace,
}: {
  section: 'profiles' | 'runs' | 'environments';
  navigate: (section: 'profiles' | 'runs' | 'environments') => void;
  children?: ReactNode;
  workspace: WorkspaceIdentity;
}) {
  return (
    <header className="app-header">
      <div className="brand">
        <img src="./oec-mascot.webp" alt="The Open Engine Company" />
        <span className="brand-rule" />
        <span>zeroshot</span>
      </div>
      <span className="local-label">{workspace.kind.toUpperCase()}</span>
      <nav className="app-sections" aria-label="Workspace">
        <button
          aria-current={section === 'profiles' ? 'page' : undefined}
          onClick={() => navigate('profiles')}
        >
          Profiles
        </button>
        <button
          aria-current={section === 'runs' ? 'page' : undefined}
          onClick={() => navigate('runs')}
        >
          Runs
        </button>
        <button
          aria-current={section === 'environments' ? 'page' : undefined}
          onClick={() => navigate('environments')}
        >
          Environments
        </button>
      </nav>
      <ThemeControl />
      {children}
    </header>
  );
}
