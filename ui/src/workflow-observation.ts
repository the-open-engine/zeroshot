import { allNodes, executable, isGroup, pathTo, type GraphNode } from './domain';

/** Cursor-specific presentation supplied by the observer, not inferred by the canvas. */
export type WorkflowNodeObservation = {
  state:
    'queued' | 'idle' | 'running' | 'suspended' | 'succeeded' | 'failed' | 'skipped' | 'recorded';
  count?: number;
  countUnit?: 'execution' | 'visit';
  detail?: string;
};

export type WorkflowObservation = {
  /** Stable run identity. Changing the cursor must not change this key. */
  key: string;
  nodes: Readonly<Record<string, WorkflowNodeObservation>>;
  /** The view follows a live run's latest events; only then does new work unfold its groups. */
  following?: boolean;
};

/** Sequences remain invisible wiring, just as in the editor; fold semantic groups only. */
export function collapsedWorkflowGroups(root: GraphNode): Set<string> {
  return new Set(
    allNodes(root)
      .filter((node) => isGroup(node) && node.kind !== 'seq')
      .map((node) => node.name)
  );
}

const currentStates = new Set<WorkflowNodeObservation['state']>(['running', 'suspended', 'failed']);

/** The executable nodes whose work is current or failed: what a run view must not hide. */
export function currentRunWork(
  root: GraphNode,
  nodes: Readonly<Record<string, WorkflowNodeObservation>> = {}
): string[] {
  return allNodes(root)
    .filter((node) => executable(node) && currentStates.has(nodes[node.name]?.state))
    .map((node) => node.name);
}

/** Unfold the groups holding `names`; the same set comes back when nothing was folded. */
export function revealRunWork(
  collapsed: Set<string>,
  root: GraphNode,
  names: readonly string[]
): Set<string> {
  const next = names.reduce((value, name) => revealWorkflowNode(value, root, name), collapsed);
  return next.size === collapsed.size ? collapsed : next;
}

/** A run opens folded, except for the groups that hold its current or failed work. */
export function collapsedRunGroups(
  root: GraphNode,
  nodes: Readonly<Record<string, WorkflowNodeObservation>> = {}
): Set<string> {
  return revealRunWork(collapsedWorkflowGroups(root), root, currentRunWork(root, nodes));
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
