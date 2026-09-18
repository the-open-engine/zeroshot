import type { Node, NodeChange } from '@xyflow/react';

type Size = { width: number; height: number };
export type WorkflowMeasurements = Record<string, Size>;

/** Retain React Flow's measured sizes when rebuilding controlled presentation nodes. */
export function updateWorkflowMeasurements(
  previous: WorkflowMeasurements,
  changes: NodeChange[]
): WorkflowMeasurements {
  let next = previous;
  for (const change of changes) {
    if (change.type !== 'dimensions' || !change.dimensions) continue;
    const { width, height } = change.dimensions;
    if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) continue;
    if (next[change.id]?.width === width && next[change.id]?.height === height) continue;
    if (next === previous) next = { ...previous };
    next[change.id] = { width, height };
  }
  return next;
}

export function withWorkflowDimensions<T extends Node>(
  node: T,
  size: Size,
  measurements: WorkflowMeasurements
): T {
  return {
    ...node,
    ...size,
    style: { ...node.style, ...size },
    measured: measurements[node.id],
  };
}
