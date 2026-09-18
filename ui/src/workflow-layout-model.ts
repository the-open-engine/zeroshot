import type { ElkNode } from 'elkjs/lib/elk-api';
import type { Positions } from './domain';
import { shortWorkflowLabel } from './workflow-display';
import type { WorkflowProjection } from './workflow-projection';

export type WorkflowDimensions = Record<string, { width: number; height: number }>;
export type WorkflowRectangle = { x: number; y: number; width: number; height: number };
export type WorkflowLayoutResult = {
  positions: Positions;
  regions: Record<string, WorkflowRectangle>;
};

/** The browser worker and headless layout checks use the same compound graph. */
export function buildWorkflowLayout(
  projection: WorkflowProjection,
  dimensions: WorkflowDimensions
): ElkNode {
  const options = {
    'elk.algorithm': 'layered',
    'elk.direction': 'RIGHT',
    'elk.hierarchyHandling': 'INCLUDE_CHILDREN',
    'elk.spacing.nodeNode': '54',
    'elk.layered.spacing.nodeNodeBetweenLayers': '42',
    'elk.layered.nodePlacement.strategy': 'BRANDES_KOEPF',
    'elk.padding': '[top=52,left=26,bottom=36,right=26]',
  };
  const regions = [...projection.regions].sort((a, b) => a.nodeIds.length - b.nodeIds.length);
  const regionNodes = new Map<string, ElkNode>(
    regions.map((region) => [region.id, { id: region.id, children: [], layoutOptions: options }])
  );
  const roots: ElkNode[] = [];
  for (const [index, region] of regions.entries()) {
    // Projection emits regions inside out. Equal member sets still represent
    // distinct scopes, so the next containing region is their parent too.
    const parent = regions
      .slice(index + 1)
      .find((other) => region.nodeIds.every((id) => other.nodeIds.includes(id)));
    (parent ? regionNodes.get(parent.id)!.children! : roots).push(regionNodes.get(region.id)!);
  }
  for (const node of projection.nodes) {
    const parent = regions.find((region) => region.nodeIds.includes(node.id));
    (parent ? regionNodes.get(parent.id)!.children! : roots).push({
      id: node.id,
      ...dimensions[node.id],
    });
  }
  return {
    id: 'workflow',
    layoutOptions: options,
    children: roots,
    edges: projection.edges
      .filter((edge) => !edge.repeat)
      .map((edge) => ({
        id: edge.id,
        sources: [edge.source],
        targets: [edge.target],
        ...(edge.label
          ? {
              labels: [
                {
                  text: shortWorkflowLabel(edge),
                  width: Math.min(118, (shortWorkflowLabel(edge)?.length ?? 0) * 6),
                  height: 24,
                },
              ],
            }
          : {}),
      })),
  };
}

/** Convert every compound-local coordinate to the canvas coordinate system. */
export function readWorkflowLayout(
  layout: ElkNode,
  projection: WorkflowProjection
): WorkflowLayoutResult {
  const nodeIds = new Set(projection.nodes.map((node) => node.id));
  const regionIds = new Set(projection.regions.map((region) => region.id));
  const result: WorkflowLayoutResult = { positions: {}, regions: {} };
  function collect(node: ElkNode, parentX: number, parentY: number) {
    const x = parentX + (node.x ?? 0);
    const y = parentY + (node.y ?? 0);
    if (nodeIds.has(node.id)) result.positions[node.id] = { x, y };
    if (regionIds.has(node.id)) {
      result.regions[node.id] = { x, y, width: node.width ?? 0, height: node.height ?? 0 };
    }
    node.children?.forEach((child) => collect(child, x, y));
  }
  layout.children?.forEach((node) => collect(node, 0, 0));
  return result;
}
