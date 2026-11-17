import type { ReactNode } from 'react';
import { X } from 'lucide-react';

/**
 * The top of every inspector, editing a profile or reading a run: where the node sits, what it is,
 * and a way to close the panel. Only what follows differs between the two.
 */
export function InspectorHeader({
  trail,
  title,
  kind,
  close,
}: {
  /** The node's enclosing groups, each a link up the hierarchy. */
  trail?: ReactNode;
  title: ReactNode;
  kind?: ReactNode;
  close: () => void;
}) {
  return (
    <header className="inspector-header">
      <div>
        {trail}
        <h2>{title}</h2>
        {kind && <p className="inspector-kind">{kind}</p>}
      </div>
      <button
        className="icon-button"
        title="Close inspector"
        aria-label="Close inspector"
        onClick={close}
      >
        <X size={17} />
      </button>
    </header>
  );
}
