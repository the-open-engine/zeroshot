// Editable wire documents remain lossless. Rust owns schema and admission; these types
// describe the fields needed by the editor, not a second protocol implementation.
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type GraphNode = { kind: string; name: string; [key: string]: any };
export type Graph = {
  profile: string;
  root: GraphNode;
  initialInput: any;
  policy: any;
  [key: string]: any;
};
export type Binding = {
  kind: string;
  model?: string;
  effort?: string;
  sessionScope?: string;
  connections?: Record<string, string[]>;
  [key: string]: any;
};
export type Runtime = {
  harness: string;
  provider: string;
  size: string;
  nodes: Record<string, Binding>;
  [key: string]: any;
};
export type Document = { name: string; graph: Graph; runtime: Runtime };
export type Template = {
  name: string;
  graph: Graph;
  id?: string;
  delivery?: string;
  label?: string;
  runtimeBindings?: Record<string, Binding>;
};
export type Point = { x: number; y: number };
export type Positions = Record<string, Point>;
export const clone = <T>(value: T): T => structuredClone(value);
export const labels: Record<string, string> = {
  step: 'Agent',
  verifier: 'Verifier',
  seq: 'Sequence',
  par: 'Parallel',
  choice: 'Choice',
  loop: 'Loop',
  map: 'Map',
  succeed: 'Success',
  fail: 'Failure',
};
export const isGroup = (node: GraphNode) =>
  ['seq', 'par', 'choice', 'loop', 'map'].includes(node.kind);
export function children(node: GraphNode): GraphNode[] {
  switch (node.kind) {
    case 'seq':
      return node.children;
    case 'par':
      return node.branches;
    case 'choice':
      return [
        ...node.branches.map((b: any) => b.node),
        ...(node.otherwise ? [node.otherwise] : []),
      ];
    case 'loop':
    case 'map':
      return [node.body];
    default:
      return [];
  }
}
export function allNodes(root: GraphNode): GraphNode[] {
  return [root, ...children(root).flatMap(allNodes)];
}
export function findNode(root: GraphNode, name: string): GraphNode | undefined {
  return allNodes(root).find((n) => n.name === name);
}
export function pathTo(root: GraphNode, name: string): GraphNode[] {
  if (root.name === name) return [root];
  for (const child of children(root)) {
    const path = pathTo(child, name);
    if (path.length) return [root, ...path];
  }
  return [];
}
export const executable = (node: GraphNode) => node.kind === 'step' || node.kind === 'verifier';
export function newDocument(template: Template): Document {
  const graph = clone(template.graph);
  return {
    name: '',
    graph,
    runtime: {
      harness: '',
      provider: '',
      size: 'small',
      nodes: Object.fromEntries(
        allNodes(graph.root)
          .filter(executable)
          .map((n) => [
            n.name,
            clone(template.runtimeBindings?.[n.name] ?? { kind: 'agent', model: '' }),
          ])
      ),
    },
  };
}
export function blankDocument(template: Template): Document {
  const doc = newDocument(template);
  doc.graph.initialInput = { kind: 'record', fields: {} };
  doc.graph.root = {
    kind: 'seq',
    name: 'run',
    state: { kind: 'record', fields: {} },
    children: [],
    promotedStatePaths: [],
  };
  doc.runtime.nodes = {};
  return doc;
}
export function uniqueName(root: GraphNode, prefix: string): string {
  const names = new Set(allNodes(root).map((n) => n.name));
  // A deleted definition can still have authored references awaiting repair. Reusing
  // that identity would silently attach those references to an unrelated new node.
  const pending: any[] = [root];
  while (pending.length) {
    const value = pending.pop();
    if (!value || typeof value !== 'object') continue;
    if (typeof value.node === 'string' && 'channel' in value) names.add(value.node);
    if (typeof value.name === 'string' && ['signal', 'error', 'group'].includes(value.source))
      names.add(value.name);
    for (const child of Object.values(value))
      if (child && typeof child === 'object') pending.push(child);
  }
  if (!names.has(prefix)) return prefix;
  let suffix = 2;
  while (names.has(`${prefix}_${suffix}`)) suffix++;
  return `${prefix}_${suffix}`;
}
export function uniqueAgentReference(root: GraphNode, name: string): string {
  // Node names may change while their worker contract identities remain authored and stable.
  // Reserve references separately so a new Agent never shares a renamed node's contract.
  const reserved = new Set(
    allNodes(root)
      .filter(executable)
      .map((node) => node.worker)
  );
  let reference = `agent.${name}@1`;
  for (let suffix = 2; reserved.has(reference); suffix++) reference = `agent.${name}.${suffix}@1`;
  return reference;
}
function rewriteReferences(value: any, before: string, after: string): void {
  if (!value || typeof value !== 'object') return;
  // Node-output selectors and control selectors are the only name-bearing references.
  if (value.node === before && 'channel' in value) value.node = after;
  if (value.name === before && ['signal', 'error', 'group'].includes(value.source))
    value.name = after;
  Object.values(value).forEach((v) => {
    if (typeof v === 'object') rewriteReferences(v, before, after);
  });
}
export function replaceNode(document: Document, name: string, replacement: GraphNode): Document {
  const next = clone(document),
    old = findNode(next.graph.root, name);
  if (!old) throw new Error('This node no longer exists.');
  if (replacement.name !== name && findNode(next.graph.root, replacement.name))
    throw new Error('A node with this name already exists.');
  const oldNames = allNodes(old)
    .filter(executable)
    .map((n) => n.name);
  Object.keys(old).forEach((k) => delete old[k]);
  Object.assign(old, clone(replacement));
  if (replacement.name !== name) {
    if (Object.hasOwn(next.runtime.nodes, name)) {
      next.runtime.nodes = { ...next.runtime.nodes, [replacement.name]: next.runtime.nodes[name] };
      delete next.runtime.nodes[name];
    }
    rewriteReferences(next.graph, name, replacement.name);
  }
  const names = new Set(
    allNodes(next.graph.root)
      .filter(executable)
      .map((n) => n.name)
  );
  oldNames.forEach((n) => {
    if (!names.has(n)) delete next.runtime.nodes[n];
  });
  allNodes(replacement)
    .filter(executable)
    .forEach((n) => {
      if (!Object.hasOwn(next.runtime.nodes, n.name))
        next.runtime.nodes = { ...next.runtime.nodes, [n.name]: { kind: 'agent', model: '' } };
    });
  return next;
}
export function reorder(
  document: Document,
  parent: string,
  name: string,
  offset: number
): Document {
  const next = clone(document),
    group = findNode(next.graph.root, parent);
  if (!group || !['seq', 'par', 'choice'].includes(group.kind))
    throw new Error('Open a sequence or branch group to reorder its nodes.');
  const list = group.kind === 'seq' ? group.children : group.branches;
  const index = list.findIndex(
      (n: any) => (group.kind === 'choice' ? n.node.name : n.name) === name
    ),
    dest = index + offset;
  if (index < 0 || dest < 0 || dest >= list.length) return next;
  list.splice(dest, 0, ...list.splice(index, 1));
  return next;
}
export function connectAfter(
  document: Document,
  parent: string,
  source: string,
  target: string
): Document {
  if (source === target) throw new Error('A node cannot follow itself.');
  const next = clone(document),
    group = findNode(next.graph.root, parent);
  if (group?.kind !== 'seq') throw new Error('Connections set execution order inside a sequence.');
  const a = group.children.findIndex((n: GraphNode) => n.name === source),
    b = group.children.findIndex((n: GraphNode) => n.name === target);
  if (a < 0 || b < 0) throw new Error('Connect nodes in the same sequence.');
  const [node] = group.children.splice(b, 1);
  const at = group.children.findIndex((n: GraphNode) => n.name === source);
  group.children.splice(at + 1, 0, node);
  return next;
}
export function removeNode(document: Document, parent: string, name: string): Document {
  const next = clone(document),
    group = findNode(next.graph.root, parent),
    node = findNode(next.graph.root, name);
  if (!node || !group || name === next.graph.root.name)
    throw new Error('The root node cannot be removed.');
  const list =
    group.kind === 'seq'
      ? group.children
      : group.kind === 'par' || group.kind === 'choice'
        ? group.branches
        : undefined;
  if (group.kind === 'choice' && group.otherwise?.name === name) group.otherwise = null;
  else {
    if (!list) throw new Error('A loop or map must have a body. Use Replace body to change it.');
    const index = list.findIndex(
      (n: any) => (group.kind === 'choice' ? n.node.name : n.name) === name
    );
    if (index < 0) throw new Error('This node is not in the open group.');
    list.splice(index, 1);
  }
  allNodes(node)
    .filter(executable)
    .forEach((n) => delete next.runtime.nodes[n.name]);
  return next;
}
export function addNode(
  document: Document,
  parent: string,
  kind: string,
  after?: string,
  otherwise = false
): { document: Document; name: string; parent: string } {
  let next = clone(document);
  let group = findNode(next.graph.root, parent);
  if (!group || !isGroup(group)) throw new Error('Open a group to add nodes.');
  // A loop/map owns one body. Adding steps edits its sequence, wrapping an existing
  // single node once while retaining its identity, runtime, and exposed state writes.
  if (['loop', 'map'].includes(group.kind)) {
    if (group.body.kind !== 'seq') next = wrapBody(next, parent).document;
    group = findNode(next.graph.root, parent)!.body as GraphNode;
  }
  const node = makeNode(next.graph.root, kind, group.state);
  if (otherwise) {
    if (group.kind !== 'choice' || group.otherwise)
      throw new Error('Remove the existing otherwise branch before replacing it.');
    group.otherwise = node;
  } else if (group.kind === 'choice') {
    const candidate = allNodes(next.graph.root).find(executable);
    group.branches.push({
      when: {
        kind: 'in',
        value: { name: candidate?.name ?? '', source: 'error', field: null },
        labels: ['crash', 'malformed', 'refusal', 'timeout'],
      },
      node,
    });
  } else {
    const list = group.kind === 'seq' ? group.children : group.branches;
    const index = list.findIndex((n: GraphNode) => n.name === after);
    list.splice(index < 0 ? list.length : index + 1, 0, node);
  }
  allNodes(node)
    .filter(executable)
    .forEach((n) => {
      next.runtime.nodes = { ...next.runtime.nodes, [n.name]: { kind: 'agent', model: '' } };
    });
  return { document: next, name: node.name, parent: group.name };
}
export const editableKinds = [
  'step',
  'verifier',
  'seq',
  'par',
  'choice',
  'loop',
  'map',
  'succeed',
  'fail',
];
function makeNode(root: GraphNode, kind: string, parentState: any): GraphNode {
  const name = uniqueName(root, kind === 'step' ? 'agent' : kind);
  const state = clone(parentState ?? { kind: 'null' });
  const simple = (n: string): GraphNode => ({
    kind: 'succeed',
    name: n,
    output: { kind: 'null' },
    bindings: [],
  });
  let node: GraphNode;
  if (kind === 'step' || kind === 'verifier') {
    node = {
      kind,
      name,
      worker: uniqueAgentReference(root, name),
      input: { kind: 'null' },
      output: { kind: 'null' },
      inputBindings: [],
      writeBindings: [],
      attempts: 1,
      instructions: 'Complete your assigned task.',
    };
    if (kind === 'verifier')
      Object.assign(node, {
        signals: { verdict: ['accepted', 'rejected'] },
        diagnostic: { kind: 'null' },
      });
  } else if (kind === 'succeed') node = simple(name);
  else if (kind === 'fail') node = { kind, name, reason: 'failed' };
  else if (kind === 'seq' || kind === 'par' || kind === 'choice')
    node = {
      kind,
      name,
      state,
      promotedStatePaths: [],
      [kind === 'seq' ? 'children' : 'branches']: [],
      ...(kind === 'par' ? { join: { kind: 'all' } } : {}),
    };
  else if (kind === 'loop')
    node = {
      kind,
      name,
      state,
      body: {
        kind: 'seq',
        name: uniqueName(root, `${name}_body`),
        state: clone(state),
        children: [],
        promotedStatePaths: [],
      },
      maxIterations: 3,
      promotedStatePaths: [],
    };
  else if (kind === 'map') {
    const collection =
      Object.entries(state?.fields ?? {}).find(
        ([, field]: [string, any]) => field?.type?.kind === 'array'
      )?.[0] ?? 'items';
    node = {
      kind,
      name,
      state,
      body: {
        kind: 'seq',
        name: uniqueName(root, `${name}_body`),
        state: clone(state),
        children: [],
        promotedStatePaths: [],
      },
      over: { source: 'state', path: [collection] },
      maxItems: 10,
      promotedStatePaths: [],
    };
  } else throw new Error('Choose a supported node type.');
  return node;
}
export function replaceBody(document: Document, parent: string, kind: string): Document {
  const group = findNode(document.graph.root, parent);
  if (!group || !['loop', 'map'].includes(group.kind)) throw new Error('Select a loop or map.');
  const body = makeNode(document.graph.root, kind, group.state);
  // Replacing a body is not renaming its old node. Existing references stay explicit
  // and Rust identifies any references that no longer resolve.
  return replaceNode(document, parent, { ...group, body });
}
export function wrapBody(document: Document, parent: string): { document: Document; name: string } {
  const group = findNode(document.graph.root, parent);
  if (!group || !['loop', 'map'].includes(group.kind)) throw new Error('Select a loop or map.');
  const name = uniqueName(document.graph.root, `${group.name}_body`);
  const exposed = isGroup(group.body)
    ? (group.body.promotedStatePaths ?? [])
    : (group.body.writeBindings ?? []).map((binding: any) => binding.target);
  const promotedStatePaths = [
    ...new Map(exposed.map((path: any) => [JSON.stringify(path), clone(path)])).values(),
  ];
  const body = {
    kind: 'seq',
    name,
    state: clone(group.state),
    promotedStatePaths,
    children: [clone(group.body)],
  };
  return { document: replaceNode(document, parent, { ...group, body }), name };
}
export function assertDocument(value: any): asserts value is Document {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('A profile must be a JSON object.');
  for (const key of Object.keys(value))
    if (!['name', 'graph', 'runtime', 'id', 'scope', 'isDefault'].includes(key))
      throw new Error(`Unknown profile field: ${key}.`);
  if (value.name !== undefined && typeof value.name !== 'string')
    throw new Error('Profile name must be text.');
  if (
    !value.graph ||
    !value.runtime ||
    typeof value.runtime !== 'object' ||
    Array.isArray(value.runtime) ||
    !value.runtime.nodes ||
    Array.isArray(value.runtime.nodes)
  )
    throw new Error('Include both graph and runtime objects, with runtime.nodes.');
  if (typeof value.runtime.nodes !== 'object')
    throw new Error('runtime.nodes must be an object of node bindings.');
  for (const key of ['harness', 'provider', 'size'])
    if (typeof value.runtime[key] !== 'string') throw new Error(`Runtime ${key} must be text.`);
  for (const [name, binding] of Object.entries(value.runtime.nodes) as [string, any][]) {
    if (
      !binding ||
      typeof binding !== 'object' ||
      Array.isArray(binding) ||
      typeof binding.kind !== 'string'
    )
      throw new Error(`Runtime binding for ${name} must be an object with a kind.`);
    for (const key of ['model', 'effort', 'sessionScope'])
      if (binding[key] !== undefined && typeof binding[key] !== 'string')
        throw new Error(`${key} for ${name} must be text.`);
  }
  let count = 0;
  const names = new Set<string>();
  function inspect(n: any, depth: number): void {
    if (++count > 500 || depth > 48)
      throw new Error('The editor supports up to 500 nodes and 48 nesting levels.');
    if (
      !n ||
      typeof n !== 'object' ||
      typeof n.kind !== 'string' ||
      !Object.hasOwn(labels, n.kind) ||
      typeof n.name !== 'string' ||
      !n.name
    )
      throw new Error('Every graph node needs a supported kind and a name.');
    if (n.instructions !== undefined && typeof n.instructions !== 'string')
      throw new Error(`Instructions for ${n.name} must be text.`);
    if (n.kind === 'fail' && typeof n.reason !== 'string')
      throw new Error(`Failure reason for ${n.name} must be text.`);
    for (const key of ['attempts', 'timeoutMs', 'maxIterations', 'maxItems'])
      if (n[key] !== undefined && typeof n[key] !== 'number' && n[key] !== '')
        throw new Error(`${key} for ${n.name} must be a number.`);
    if (names.has(n.name)) throw new Error(`Duplicate node name: ${n.name}.`);
    names.add(n.name);
    if (n.kind === 'seq' && !Array.isArray(n.children))
      throw new Error('A sequence needs a children array.');
    if (['par', 'choice'].includes(n.kind) && !Array.isArray(n.branches))
      throw new Error('A branch group needs a branches array.');
    if (n.kind === 'choice' && n.branches.some((b: any) => !b || !b.node))
      throw new Error('Each choice branch needs a node.');
    children(n).forEach((child) => inspect(child, depth + 1));
  }
  inspect(value.graph.root, 0);
}

export function bindingFor(runtime: Runtime, name: string): Binding | undefined {
  return Object.hasOwn(runtime.nodes, name) ? runtime.nodes[name] : undefined;
}
