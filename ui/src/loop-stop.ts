import { allNodes, type GraphNode } from './domain';

/** Restrict selector kinds/scope; native admission proves completion availability. */
export function loopStopCandidates(loop?: GraphNode): GraphNode[] {
  if (loop?.kind !== 'loop') return [];
  // A verifier in an otherwise branch can be definite on every completing round
  // when the other outcome terminates the run. Do not approximate that proof here.
  return loop.body ? allNodes(loop.body).filter((node) => node.kind === 'verifier') : [];
}
