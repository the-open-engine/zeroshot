import { Component, type ReactNode } from 'react';
import { AppHeader } from './AppHeader';
import type { WorkspaceIdentity } from './api';

/** A damaged history view must not discard the editor's independently retained draft. */
export class RunHistoryBoundary extends Component<
  { children: ReactNode; workspace: WorkspaceIdentity; embedded?: boolean },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    if (!this.state.failed) return this.props.children;
    return (
      <div className="app history-app">
        {!this.props.embedded && (
          <AppHeader
            section="runs"
            workspace={this.props.workspace}
            navigate={(section) => {
              window.location.hash = section === 'profiles' ? '' : 'runs';
            }}
          />
        )}
        <main className="history-empty">
          <h1>Could not display this history</h1>
          <p>The profile draft is still available in Profiles.</p>
          <button
            className="button"
            onClick={() => {
              if (!this.props.embedded) window.location.hash = 'runs';
              this.setState({ failed: false });
            }}
          >
            Reopen run history
          </button>
        </main>
      </div>
    );
  }
}
