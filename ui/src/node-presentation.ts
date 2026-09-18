import {
  Bot,
  ShieldCheck,
  GitPullRequest,
  Layers,
  Columns3,
  Split,
  Repeat2,
  ListTree,
  Check,
  X,
} from 'lucide-react';
import { bindingFor, executable, labels, type Document, type GraphNode } from './domain';
import { workerOption, type WorkerOption } from './workers';

export const nodeIcons: Record<string, typeof Bot> = {
  step: Bot,
  verifier: ShieldCheck,
  seq: Layers,
  par: Columns3,
  choice: Split,
  loop: Repeat2,
  map: ListTree,
  succeed: Check,
  fail: X,
  git_delivery: GitPullRequest,
};

export function nodePresentation(document: Document, node: GraphNode, workers: WorkerOption[]) {
  const binding = bindingFor(document.runtime, node.name);
  const worker = workerOption(node, binding, workers);
  const delivery =
    executable(node) &&
    (worker?.runtimeKind === 'git_delivery' || binding?.kind === 'git_delivery');
  return {
    Icon: nodeIcons[delivery ? 'git_delivery' : node.kind] ?? Layers,
    label: delivery ? 'Git delivery' : labels[node.kind],
    delivery,
    detail: delivery
      ? worker?.id === 'git_delivery_pr'
        ? 'Pull request'
        : worker?.id === 'git_delivery_merge'
          ? 'Merge'
          : 'Git delivery'
      : binding?.model,
  };
}
