import { useEffect, useRef } from 'react';
import { Menu } from 'lucide-react';
import { ThemeControl } from './ThemeControl';
import type { WorkspaceIdentity } from './api';

function workspaceKindLabel(kind: string) {
  return kind.charAt(0).toUpperCase() + kind.slice(1);
}

export type PageSidebar = {
  label: string;
  /** The sidebar element's id. */
  id: string;
  open: boolean;
  setOpen: (open: boolean) => void;
};

/**
 * Opens the page's sidebar where it is folded away (medium and narrow layouts). Escape closes the
 * overlaid sidebar from inside it and returns focus here.
 */
function SidebarToggle({ label, id, open, setOpen }: PageSidebar) {
  const button = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const escape = (event: KeyboardEvent) => {
      const toggle = button.current;
      // A docked sidebar has no toggle to return to. Escape elsewhere (a dialog, a field) is theirs.
      if (event.key !== 'Escape' || !toggle || window.getComputedStyle(toggle).display === 'none')
        return;
      const target = event.target instanceof Node ? event.target : null;
      const sidebar = document.getElementById(id);
      if (target !== document.body && !sidebar?.contains(target) && !toggle.contains(target))
        return;
      setOpen(false);
      toggle.focus();
    };
    document.addEventListener('keydown', escape);
    return () => document.removeEventListener('keydown', escape);
  }, [open, id, setOpen]);
  return (
    <button
      ref={button}
      className="icon-button sidebar-toggle"
      title={label}
      aria-label={label}
      aria-expanded={open}
      aria-controls={id}
      onClick={() => setOpen(!open)}
    >
      <Menu size={18} />
    </button>
  );
}

export function AppHeader({
  section,
  navigate,
  sidebar,
  workspace,
}: {
  section: 'profiles' | 'runs';
  navigate: (section: 'profiles' | 'runs') => void;
  /** The page's sidebar, if any; its menu button leads, on the side the sidebar opens. */
  sidebar?: PageSidebar;
  workspace: WorkspaceIdentity;
}) {
  return (
    <header className="app-header">
      {sidebar && <SidebarToggle {...sidebar} />}
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
