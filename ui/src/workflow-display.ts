import type { WorkflowEdge, WorkflowProjection } from './workflow-projection';

export type WorkflowDisplayEdge = WorkflowEdge & {
  /** Compact caption only; label remains the original projection text. */
  shortLabel?: string;
  fullLabel?: string;
};
export type WorkflowDisplayProjection = Omit<WorkflowProjection, 'edges'> & {
  edges: WorkflowDisplayEdge[];
};

const allErrors = new Set(['timeout', 'crash', 'malformed', 'refusal']);

function allErrorSource(text: string): string | undefined {
  const match = /^(.+) error is (.+)$/.exec(text);
  if (!match) return undefined;
  const labels = match[2].split(' | ');
  return labels.length === 4 &&
    new Set(labels).size === 4 &&
    labels.every((label) => allErrors.has(label))
    ? match[1]
    : undefined;
}

// Only recognize flat, complete expressions emitted by guardLabel. Truncated,
// nested, mixed, or unfamiliar guards retain their original caption.
function operands(text: string, operator: 'OR' | 'AND'): string[] | undefined {
  if (!text.startsWith('(') || !text.endsWith(')')) return undefined;
  const parts = text.slice(1, -1).split(`) ${operator} (`);
  if (parts.length < 2 || parts.some((part) => /[()…]/.test(part))) return undefined;
  return parts;
}

export function shortWorkflowLabel(edge: WorkflowEdge): string | undefined {
  if (edge.shortLabel !== undefined) return edge.shortLabel;
  if (edge.label === undefined) return undefined;
  if (edge.otherwise && edge.label === 'OTHERWISE · no earlier match') return 'Otherwise';
  const numbered = /^(\d+\. )(.*)$/.exec(edge.label);
  const priority = numbered?.[1] ?? '';
  const text = numbered?.[2] ?? edge.label;
  const source = allErrorSource(text);
  if (source) return `${priority}${source} has an error`;
  const alternatives = operands(text, 'OR');
  if (alternatives?.every((part) => allErrorSource(part) !== undefined))
    return `${priority}Any error`;
  const conjunction = operands(text, 'AND');
  if (conjunction?.every((part) => /^.+ · verdict is accepted$/.test(part)))
    return `${priority}All accepted`;
  return edge.label;
}

/** Compact virtual presentation only. Authored owners, conditions, and ordering survive. */
export function compactWorkflow(projection: WorkflowProjection): WorkflowDisplayProjection {
  const nodes = new Set(projection.nodes.map((node) => node.id));
  const entries = new Map<string, string>();
  for (const node of projection.nodes) {
    if (node.role !== 'loop-start') continue;
    if (
      !projection.regions.some(
        (region) =>
          region.kind === 'loop' && region.owner === node.owner && region.nodeIds.includes(node.id)
      )
    )
      continue;
    const outgoing = projection.edges.filter((edge) => edge.source === node.id);
    if (outgoing.length !== 1) continue;
    const entry = outgoing[0];
    // The region replaces the plain entry marker, never an authored condition.
    if (
      entry.repeat ||
      entry.secondary ||
      entry.otherwise ||
      entry.branchIndex !== undefined ||
      (entry.label !== undefined && entry.label !== 'First round')
    )
      continue;
    if (nodes.has(entry.target)) entries.set(node.id, entry.target);
  }
  function destination(id: string): string | undefined {
    const seen = new Set<string>();
    while (entries.has(id)) {
      if (seen.has(id)) return undefined;
      seen.add(id);
      id = entries.get(id)!;
    }
    return id;
  }
  const removed = new Set([...entries.keys()].filter((id) => destination(id) !== undefined));
  return {
    ...projection,
    nodes: projection.nodes.filter((node) => !removed.has(node.id)).map((node) => ({ ...node })),
    edges: projection.edges
      .filter((edge) => !removed.has(edge.source))
      .map((edge) => ({
        ...edge,
        target: removed.has(edge.target) ? destination(edge.target)! : edge.target,
        ...(edge.label === undefined
          ? {}
          : { fullLabel: edge.label, shortLabel: shortWorkflowLabel(edge) }),
      })),
    regions: projection.regions.map((region) => ({
      ...region,
      nodeIds: region.nodeIds.filter((id) => !removed.has(id)),
    })),
  };
}
