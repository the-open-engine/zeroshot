import { useMemo } from 'react';
import { ChevronRight } from 'lucide-react';
import type { Document } from './domain';
import { drawnAncestors, drawnOwners, nodeTitle } from './workflow-projection';

/**
 * Where a node sits in its graph: each enclosing group the canvas draws, up to the root, named as
 * its card is, and each opening in the inspector. The node itself is the inspector's title, so it
 * is not repeated.
 */
export function NodeTrail({
  document,
  name,
  rootLabel,
  select,
}: {
  document: Document;
  name: string;
  /** The title the root's inspector shows, so following the crumb lands on the same name. */
  rootLabel: string;
  select: (name: string) => void;
}) {
  const drawn = useMemo(() => drawnOwners(document), [document]);
  const ancestors = drawnAncestors(document, name, drawn);
  if (!ancestors.length) return null;
  const root = document.graph.root.name;
  return (
    <nav className="node-trail" aria-label="Location in graph">
      <ol>
        {ancestors.map((ancestor, index) => {
          const label = ancestor.name === root ? rootLabel : nodeTitle(ancestor.name);
          return (
            <li key={ancestor.name}>
              {index > 0 && <ChevronRight size={11} aria-hidden="true" />}
              <button type="button" title={label} onClick={() => select(ancestor.name)}>
                {label}
              </button>
            </li>
          );
        })}
      </ol>
    </nav>
  );
}
