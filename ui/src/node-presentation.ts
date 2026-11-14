import {
  Bot,
  ShieldCheck,
  GitPullRequest,
  MessageSquare,
  Clock,
  Layers,
  Columns3,
  Split,
  Repeat2,
  ListTree,
  Check,
  X,
} from 'lucide-react';
import {
  bindingFor,
  executable,
  labels,
  type Binding,
  type Document,
  type GraphNode,
} from './domain';
import { isDeliveryKind, scriptCommandLabel, workerOption, type WorkerOption } from './workers';

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
  request: Clock,
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
      nodeIcons[
        delivery
          ? comment
            ? 'comment_delivery'
            : 'git_delivery'
          : binding?.kind === 'request'
            ? 'request'
            : node.kind
      ] ?? Layers,
    label: delivery
      ? comment
        ? 'Comment delivery'
        : 'Git delivery'
      : activityLabel(node.kind, binding?.kind),
    delivery,
    detail: delivery ? deliveryDetail(worker, comment) : activityDetail(binding),
  };
}

function deliveryDetail(worker: WorkerOption | undefined, comment: boolean): string {
  if (comment) return 'Conversation comment';
  if (worker?.id === 'git_delivery_pr') return 'Pull request';
  if (worker?.id === 'git_delivery_merge') return 'Merge';
  return 'Git delivery';
}

function activityDetail(binding: Binding | undefined): string | undefined {
  if (binding?.kind === 'request') return 'Wait';
  return binding?.kind === 'script' ? scriptCommandLabel(binding.command) : binding?.model;
}

export function activityLabel(nodeKind: string, bindingKind?: string): string {
  if (bindingKind === 'script') return 'Script';
  if (bindingKind === 'request') return 'Wait';
  return bindingKind === 'decision' ? 'Decision' : labels[nodeKind];
}
