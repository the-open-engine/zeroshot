// Choices describe the native host's supported implementations. Their delivery contracts and
// bindings come from bootstrap; authored Agent references remain graph-local identities.
import {
  allNodes,
  bindingFor,
  clone,
  executable,
  findNode,
  replaceNode,
  uniqueAgentReference,
  type Binding,
  type Document,
  type GraphNode,
} from './domain';

export type WorkerOption = {
  id: string;
  label: string;
  runtimeKind: 'agent' | 'git_delivery' | 'comment_delivery';
  workerRefs: string[];
  node?: GraphNode;
  runtimeBinding: Binding;
};

export function workerOption(
  node: GraphNode,
  binding: Binding | undefined,
  workers: WorkerOption[]
): WorkerOption | undefined {
  const delivery = workers.find(
    (option) => isDeliveryKind(option.runtimeKind) && option.workerRefs.includes(node.worker)
  );
  if (delivery) return delivery;
  return binding?.kind === 'agent'
    ? workers.find((option) => option.runtimeKind === 'agent')
    : undefined;
}

export function applyWorker(
  document: Document,
  name: string,
  option: WorkerOption,
  workers: WorkerOption[] = []
): Document {
  const node = findNode(document.graph.root, name);
  if (!node || !executable(node)) throw new Error('Select an Agent or Verifier node.');
  const binding = bindingFor(document.runtime, name);
  const current = workerOption(node, binding, workers.length ? workers : [option]);
  if (current?.id === option.id && binding?.kind === option.runtimeKind) return document;
  let replacement: GraphNode;
  if (isDeliveryKind(option.runtimeKind)) {
    if (!option.node || option.node.kind !== 'verifier')
      throw new Error('The native delivery contract is unavailable. Reload the editor.');
    const anotherDelivery = allNodes(document.graph.root).some(
      (other) =>
        other.name !== name &&
        executable(other) &&
        (isDeliveryKind(bindingFor(document.runtime, other.name)?.kind) ||
          isDeliveryKind(
            workerOption(other, bindingFor(document.runtime, other.name), workers)?.runtimeKind
          ))
    );
    if (anotherDelivery) throw new Error('A profile can contain only one delivery worker.');
    replacement = {
      ...clone(option.node),
      name,
      inputBindings: [],
      writeBindings: [],
    };
    delete replacement.instructions;
  } else {
    replacement = {
      ...clone(node),
      worker: uniqueAgentReference(document.graph.root, name),
      instructions: node.instructions ?? 'Complete your assigned task.',
    };
  }
  const next = replaceNode(document, name, replacement);
  next.runtime.nodes = { ...next.runtime.nodes, [name]: clone(option.runtimeBinding) };
  return next;
}

export function isDeliveryKind(kind: unknown): boolean {
  return kind === 'git_delivery' || kind === 'comment_delivery';
}
