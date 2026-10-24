import {
  Bot,
  ShieldCheck,
  GitPullRequest,
  MessageSquare,
  Layers,
  Columns3,
  Split,
  Repeat2,
  ListTree,
  Check,
  X,
} from 'lucide-react';
import { bindingFor, executable, labels, type Document, type GraphNode } from './domain';
import { isDeliveryKind, workerOption, type WorkerOption } from './workers';

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
  comment_delivery: MessageSquare,
};

export function nodePresentation(document: Document, node: GraphNode, workers: WorkerOption[]) {
  const binding = bindingFor(document.runtime, node.name);
  const worker = workerOption(node, binding, workers);
  const delivery =
    executable(node) && (isDeliveryKind(worker?.runtimeKind) || isDeliveryKind(binding?.kind));
  const comment =
    worker?.runtimeKind === 'comment_delivery' || binding?.kind === 'comment_delivery';
  return {
    Icon:
      nodeIcons[delivery ? (comment ? 'comment_delivery' : 'git_delivery') : node.kind] ?? Layers,
    label: delivery ? (comment ? 'Comment delivery' : 'Git delivery') : labels[node.kind],
    delivery,
    detail: delivery ? deliveryDetail(worker, comment) : binding?.model,
  };
}

function deliveryDetail(worker: WorkerOption | undefined, comment: boolean): string {
  if (comment) return 'Conversation comment';
  if (worker?.id === 'git_delivery_pr') return 'Pull request';
  if (worker?.id === 'git_delivery_merge') return 'Merge';
  return 'Git delivery';
}
