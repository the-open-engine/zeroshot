import { allNodes, isGroup, pathTo, type GraphNode } from './domain';

/** Cursor-specific presentation supplied by the observer, not inferred by the canvas. */
export type WorkflowNodeObservation = {
  state: 'idle' | 'running' | 'succeeded' | 'failed' | 'skipped' | 'recorded';
  count?: number;
  countUnit?: 'execution' | 'visit';
  detail?: string;
};

export type WorkflowObservation = {
  /** Stable run identity. Changing the cursor must not change this key. */
  key: string;
  nodes: Readonly<Record<string, WorkflowNodeObservation>>;
};

/** Sequences remain invisible wiring, just as in the editor; fold semantic groups only. */
export function collapsedWorkflowGroups(root: GraphNode): Set<string> {
  return new Set(
    allNodes(root)
      .filter((node) => isGroup(node) && node.kind !== 'seq')
      .map((node) => node.name)
  );
}

/** Explicit navigation reveals its path without expanding unrelated subprocesses. */
export function revealWorkflowNode(
  collapsed: Set<string>,
  root: GraphNode,
  name: string
): Set<string> {
  const next = new Set(collapsed);
  for (const ancestor of pathTo(root, name).slice(0, -1)) next.delete(ancestor.name);
  return next;
}
