import ELK from 'elkjs/lib/elk-api';
import workerUrl from 'elkjs/lib/elk-worker.min.js?url';
import {
  buildWorkflowLayout,
  readWorkflowLayout,
  type WorkflowDimensions,
  type WorkflowLayoutResult,
} from './workflow-layout-model';
import type { WorkflowProjection } from './workflow-projection';
export type { WorkflowLayoutResult, WorkflowRectangle } from './workflow-layout-model';
const elk = new ELK({ workerUrl });

/** Compound layout keeps unrelated outcomes outside repeat and parallel boundaries. */
export async function arrangeWorkflow(
  projection: WorkflowProjection,
  dimensions: WorkflowDimensions
): Promise<WorkflowLayoutResult> {
  const result = await elk.layout(buildWorkflowLayout(projection, dimensions));
  return readWorkflowLayout(result, projection);
}
