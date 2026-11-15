import type { ReactNode } from 'react';
import { ThemeControl } from './ThemeControl';
import type { WorkspaceIdentity } from './api';

function workspaceKindLabel(kind: string) {
  return kind.charAt(0).toUpperCase() + kind.slice(1);
}

export function AppHeader({
  section,
  navigate,
  sidebarToggle,
  workspace,
}: {
  section: 'profiles' | 'runs';
  navigate: (section: 'profiles' | 'runs') => void;
  /** Opens the page's sidebar where it is folded away; it leads, on the side the sidebar opens. */
  sidebarToggle?: ReactNode;
  workspace: WorkspaceIdentity;
}) {
  return (
    <header className="app-header">
      {sidebarToggle}
      <div className="brand">
        <img src="./oec-mascot.webp" alt="The Open Engine Company" />
        <span>zeroshot</span>
      </div>
      <span className="local-label">{workspaceKindLabel(workspace.kind)}</span>
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
      </nav>
      <ThemeControl />
    </header>
  );
}
