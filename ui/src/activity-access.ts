import {
  allNodes,
  assertDocument,
  bindingFor,
  executable,
  findNode,
  replaceNode,
  uniqueAgentReference,
  type Binding,
  type Document,
  type GraphNode,
} from './domain';

export function editableActivityRole(node: GraphNode, binding: Binding | undefined): boolean {
  return executable(node) && binding?.kind === 'agent' && !node.worker?.startsWith('builtin.');
}

/** Change the native execution role without changing dataflow or runtime selection. */
export function makeReadOnlyVerifier(document: Document, name: string): Document {
  assertDocument(document);
  const node = findNode(document.graph.root, name);
  if (!node || !editableActivityRole(node, bindingFor(document.runtime, name)))
    throw new Error('Choose an Agent-backed activity with an editable contract.');
  if (node.kind === 'verifier') return document;
  // Worker references identify complete contracts. A shared Step contract cannot
  // acquire verifier fields on just one node while retaining the same reference.
  const shared = allNodes(document.graph.root).some(
    (other) => other.name !== name && executable(other) && other.worker === node.worker
  );
  const next = replaceNode(document, name, {
    ...node,
    kind: 'verifier',
    worker: shared ? uniqueAgentReference(document.graph.root, name) : node.worker,
    signals: node.signals ?? {},
    diagnostic: node.diagnostic ?? { kind: 'null' },
  });
  assertDocument(next);
  return next;
}

function requireNoVerifierReferences(document: Document, name: string) {
  const pending: unknown[] = [document.graph];
  while (pending.length) {
    const value = pending.pop();
    if (!value || typeof value !== 'object') continue;
    const record = value as Record<string, unknown>;
    if (
      (record.name === name && record.source === 'signal') ||
      (record.node === name && ['signal', 'diagnostic'].includes(String(record.channel)))
    )
      throw new Error(
        'Disconnect this activity’s outcome and feedback references before making it a writing Agent.'
      );
    pending.push(...Object.values(record));
  }
}

function emptyVerifierOutputs(node: GraphNode): boolean {
  const emptySignals =
    node.signals === undefined ||
    (node.signals !== null &&
      typeof node.signals === 'object' &&
      !Array.isArray(node.signals) &&
      Object.keys(node.signals).length === 0);
  const diagnostic = node.diagnostic;
  const emptyDiagnostic =
    diagnostic === undefined ||
    (diagnostic?.kind === 'null' && Object.keys(diagnostic).length === 1) ||
    (diagnostic?.kind === 'record' &&
      Object.keys(diagnostic).every((key) => ['kind', 'fields'].includes(key)) &&
      diagnostic.fields !== null &&
      typeof diagnostic.fields === 'object' &&
      !Array.isArray(diagnostic.fields) &&
      Object.keys(diagnostic.fields).length === 0);
  return emptySignals && emptyDiagnostic;
}

/** Explicit role conversion never removes a live verifier-only data or control source. */
export function makeWritingAgent(document: Document, name: string): Document {
  assertDocument(document);
  const node = findNode(document.graph.root, name);
  if (!node || !editableActivityRole(node, bindingFor(document.runtime, name)))
    throw new Error('Choose an Agent-backed activity with an editable contract.');
  if (node.kind === 'step') return document;
  requireNoVerifierReferences(document, name);
  if (!emptyVerifierOutputs(node))
    throw new Error(
      'Remove this activity’s outcome and feedback output fields before making it a writing Agent. Regular outputs stay unchanged.'
    );
  if (node.attempts !== 1) throw new Error('Set Attempts to 1 before making this a writing Agent.');
  const shared = allNodes(document.graph.root).some(
    (other) => other.name !== name && executable(other) && other.worker === node.worker
  );
  const replacement: GraphNode = {
    ...node,
    kind: 'step',
    worker: shared ? uniqueAgentReference(document.graph.root, name) : node.worker,
  };
  delete replacement.signals;
  delete replacement.diagnostic;
  const next = replaceNode(document, name, replacement);
  assertDocument(next);
  return next;
}
