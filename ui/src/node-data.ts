import {
  allNodes,
  children,
  clone,
  executable,
  findNode,
  isGroup,
  pathTo,
  replaceNode,
  type Document,
  type GraphNode,
} from './domain';
import { bindingContext, bindingKey, fieldChoices, pathLabel, payloadAtPath } from './bindings';
import { existingDataConnections } from './data-connections';
import { analyzeWorkflowOutcomes } from './workflow-outcomes';
import { recordFields, removeField, renameField, type Payload } from './schema';

export type OutputChannel = 'out' | 'diagnostic' | 'signal';
export type NodeDataSource =
  | { kind: 'run_input'; path: string[] }
  | { kind: 'node_output'; node: string; channel: OutputChannel; path: string[] }
  | { kind: 'map_item'; path: string[] }
  | { kind: 'loop_input'; node: string; path: string[] };
export type NodeDataAction =
  | { kind: 'connect'; target: { node: string; input: string }; source: NodeDataSource }
  | { kind: 'remove_input'; target: { node: string; input: string } }
  | { kind: 'map_collection'; node: string; source: NodeDataSource }
  | { kind: 'run_input_field'; before?: string; name: string; type: Payload; required: boolean }
  | { kind: 'remove_run_input'; name: string };
export type NodeDataChoice = { id: string; source: NodeDataSource; label: string; type: Payload };
export type NodeOutputField = {
  name: string;
  type: Payload;
  required: boolean;
  channel: OutputChannel;
};
const list = (value: unknown): any[] => (Array.isArray(value) ? value : []);
const samePath = (a: unknown, b: unknown) => bindingKey(a) === bindingKey(b);
const prefix = (a: any, b: any): boolean =>
  Array.isArray(a) &&
  Array.isArray(b) &&
  a.length > 0 &&
  a.length <= b.length &&
  a.every((part, index) => typeof part === 'string' && b[index] === part);
const overlaps = (a: any, b: any) => prefix(a, b) || prefix(b, a);
const promotes = (node: GraphNode, path: string[]) =>
  list(node.promotedStatePaths).some((value) => prefix(value, path));

type DataOrigin = {
  node: string;
  channel: OutputChannel;
  path: string[];
  previous: boolean;
  scopes: string[];
};

function producerRoute(
  document: Document,
  producer: GraphNode,
  consumer: GraphNode,
  path: string[]
) {
  const a = pathTo(document.graph.root, producer.name),
    b = pathTo(document.graph.root, consumer.name);
  let shared = 0;
  while (shared < a.length && shared < b.length && a[shared].name === b[shared].name) shared++;
  let commonIndex = shared - 1;
  let previous = producer.name === consumer.name;
  const common = a[commonIndex];
  if (!common) return;
  if (common.kind === 'seq') {
    const order = children(common);
    previous ||= order.indexOf(a[shared]) >= order.indexOf(b[shared]);
  } else if (common.kind === 'par' || common.kind === 'choice' || producer.name === consumer.name)
    previous = true;
  else return;
  if (previous) {
    const loopIndex =
      a
        .slice(0, commonIndex + 1)
        .map((node, index) => (node.kind === 'loop' ? index : -1))
        .filter((index) => index >= 0)
        .at(-1) ?? -1;
    if (loopIndex < 0) return;
    // An iteration carries its body result forward only through its authored returns.
    commonIndex = loopIndex;
  }
  const outgoing = a.slice(commonIndex + 1, -1).filter(isGroup);
  if (!outgoing.every((scope) => promotes(scope, path))) return;
  const scopes = [...a.slice(commonIndex, -1), ...b.slice(commonIndex + 1, -1)].filter(isGroup);
  if (!scopes.every((scope) => payloadAtPath(scope.state, path))) return;
  return { previous, scopes: [...new Set(scopes.map((scope) => scope.name))] };
}

/** Resolve explicit writes through their real return scopes, including loop-carried values. */
export function inputOrigins(
  document: Document,
  consumer: GraphNode,
  path: string[]
): DataOrigin[] {
  const origins: DataOrigin[] = [];
  for (const writer of allNodes(document.graph.root).filter(executable)) {
    const route = producerRoute(document, writer, consumer, path);
    if (!route) continue;
    for (const binding of list(writer.writeBindings)) {
      if (
        !prefix(binding?.target, path) ||
        !Array.isArray(binding?.value?.path) ||
        binding.value.node !== writer.name ||
        !['out', 'diagnostic', 'signal'].includes(binding.value.channel)
      )
        continue;
      const sourcePath = [...binding.value.path, ...path.slice(binding.target.length)];
      if (
        !nodeOutputFields(writer).some(
          (field) => field.channel === binding.value.channel && field.name === sourcePath[0]
        )
      )
        continue;
      origins.push({
        node: writer.name,
        channel: binding.value.channel,
        path: sourcePath,
        ...route,
      });
    }
  }
  return origins.filter(
    (origin, index) =>
      origins.findIndex((candidate) => JSON.stringify(candidate) === JSON.stringify(origin)) ===
      index
  );
}

export function nodeOutputFields(node: GraphNode): NodeOutputField[] {
  const payloads: { channel: OutputChannel; value: any }[] = [
    { channel: 'out', value: node.output },
    ...(node.kind === 'verifier'
      ? [{ channel: 'diagnostic' as const, value: node.diagnostic }]
      : []),
  ];
  const fields = payloads.flatMap(({ channel, value }) =>
    Object.entries(recordFields(value) ?? {}).map(([name, field]) => ({
      name,
      type: field?.type,
      required: field?.required === true,
      channel,
    }))
  );
  if (
    node.kind === 'verifier' &&
    node.signals &&
    typeof node.signals === 'object' &&
    !Array.isArray(node.signals)
  ) {
    for (const [name, values] of Object.entries(node.signals))
      if (Array.isArray(values) && values.every((value) => typeof value === 'string'))
        fields.push({ name, type: { kind: 'enum', values }, required: true, channel: 'signal' });
  }
  return fields;
}

function beforeInSequence(document: Document, producer: GraphNode, consumer: GraphNode): boolean {
  const a = pathTo(document.graph.root, producer.name),
    b = pathTo(document.graph.root, consumer.name);
  let common = 0;
  while (common < a.length && common < b.length && a[common].name === b[common].name) common++;
  const owner = a[common - 1];
  if (!owner || owner.kind !== 'seq' || !a[common] || !b[common]) return false;
  const siblings = children(owner);
  return siblings.indexOf(a[common]) < siblings.indexOf(b[common]);
}

function continuingOutputs(node: GraphNode): NodeOutputField[] | undefined {
  if (['fail', 'succeed'].includes(node.kind)) return undefined;
  if (executable(node)) return nodeOutputFields(node).filter((field) => field.channel === 'out');
  if (node.kind !== 'seq') return [];
  const branches = children(node).map(continuingOutputs);
  if (branches.some((branch) => branch === undefined)) return undefined;
  const fields = branches.flatMap((branch) => branch!);
  return fields.filter(
    (field) => fields.filter((candidate) => candidate.name === field.name).length === 1
  );
}

/** Declared common branch results; native authoring still proves their routes. */
export function choiceOutputFields(node: GraphNode): NodeOutputField[] {
  if (node.kind !== 'choice') return [];
  const routes = children(node)
    .map(continuingOutputs)
    .filter((route): route is NodeOutputField[] => route !== undefined);
  if (!routes.length) return [];
  return routes[0].filter((field) =>
    routes.every((route) =>
      route.some(
        (candidate) =>
          candidate.name === field.name &&
          JSON.stringify(candidate.type) === JSON.stringify(field.type)
      )
    )
  );
}

export function nodeDataChoices(document: Document, consumer: GraphNode): NodeDataChoice[] {
  const result: NodeDataChoice[] = [];
  const add = (source: NodeDataSource, label: string, type: Payload) =>
    result.push({ id: JSON.stringify(source), source, label, type });
  for (const field of fieldChoices(document.graph.initialInput)) {
    const overwritten = allNodes(document.graph.root)
      .filter(executable)
      .some((node) =>
        list(node.writeBindings).some((binding) => overlaps(binding?.target, field.path))
      );
    if (!overwritten)
      add(
        { kind: 'run_input', path: field.path },
        `Run input · ${pathLabel(field.path)}`,
        field.type
      );
  }
  for (const field of fieldChoices(bindingContext(document, consumer).item))
    add({ kind: 'map_item', path: field.path }, `Item · ${pathLabel(field.path)}`, field.type);
  for (const producer of allNodes(document.graph.root)) {
    if (!beforeInSequence(document, producer, consumer)) continue;
    const ancestors = pathTo(document.graph.root, producer.name).slice(0, -1);
    const consumerAncestors = pathTo(document.graph.root, consumer.name);
    if (
      ancestors.some(
        (owner) =>
          (owner.kind === 'choice' || (owner.kind === 'par' && owner.join?.kind !== 'all')) &&
          !consumerAncestors.some((ancestor) => ancestor.name === owner.name)
      )
    )
      continue;
    if (producer.kind === 'choice') {
      for (const field of choiceOutputFields(producer))
        add(
          { kind: 'node_output', node: producer.name, channel: 'out', path: [field.name] },
          `${producer.name} · ${field.name}`,
          field.type
        );
      continue;
    }
    if (!executable(producer)) continue;
    for (const field of nodeOutputFields(producer)) {
      const parent = pathTo(document.graph.root, producer.name).slice(0, -1);
      const aggregate = parent.some(
        (node) =>
          node.kind === 'map' &&
          !pathTo(document.graph.root, consumer.name).some(
            (ancestor) => ancestor.name === node.name
          )
      );
      const type = aggregate ? { kind: 'array', items: field.type } : field.type;
      add(
        { kind: 'node_output', node: producer.name, channel: field.channel, path: [field.name] },
        `${producer.name} · ${field.name}${field.channel === 'signal' ? ' · outcome' : ''}`,
        type
      );
    }
  }
  return [...result, ...existingLoopInputChoices(document, consumer)];
}

function requiredType(schema: any, path: string[]): Payload | undefined {
  let current = schema;
  for (const part of path) {
    const field = recordFields(current)?.[part];
    if (!field?.required) return;
    current = field.type;
  }
  return current;
}

function loopScopes(document: Document, node: GraphNode, loopName: string): GraphNode[] | undefined {
  const scopes = pathTo(document.graph.root, node.name).slice(0, -1);
  return [...scopes].reverse().find((scope) => scope.kind === 'loop')?.name === loopName &&
    !scopes.some((scope) => scope.kind === 'map')
    ? scopes
    : undefined;
}

function loopInputBinding(donor: GraphNode, field: ReturnType<typeof nodeInputRows>[number]) {
  const bindings = list(donor.inputBindings).filter(
    (binding) => binding?.target?.[0] === field.name
  );
  const [binding] = bindings;
  return field.required &&
    bindings.length === 1 &&
    samePath(binding?.target, [field.name]) &&
    binding?.value?.source === 'state' &&
    Array.isArray(binding.value.path) &&
    binding.value.path.length
    ? binding
    : undefined;
}

function matchingPreviousOrigin(
  document: Document,
  donor: GraphNode,
  consumer: GraphNode,
  path: string[]
): ReturnType<typeof inputOrigins>[number] | undefined {
  const origins = inputOrigins(document, donor, path);
  const targets = inputOrigins(document, consumer, path);
  if (origins.length !== 1 || targets.length !== 1 || !origins[0].previous || !targets[0].previous)
    return undefined;
  const origin = origins[0],
    target = targets[0];
  return origin.node === target.node &&
    origin.channel === target.channel &&
    samePath(origin.path, target.path)
    ? origin
    : undefined;
}

/** Offer existing feedback routes only; selecting one never creates a recurrence. */
function existingLoopInputChoices(document: Document, consumer: GraphNode): NodeDataChoice[] {
  if (!executable(consumer)) return [];
  const ancestry = pathTo(document.graph.root, consumer.name).slice(0, -1);
  const loop = [...ancestry].reverse().find((node) => node.kind === 'loop');
  if (!loop || ancestry.some((node) => node.kind === 'map')) return [];
  const writers = allNodes(document.graph.root).filter(executable);
  const choices = new Map<string, NodeDataChoice>();
  for (const donor of writers) {
    const donorScopes = loopScopes(document, donor, loop.name);
    if (!donorScopes) continue;
    for (const field of nodeInputRows(donor)) {
      const binding = loopInputBinding(donor, field);
      if (!binding) continue;
      const path: string[] = binding.value.path;
      const origin = matchingPreviousOrigin(document, donor, consumer, path);
      if (!origin) continue;
      const writer = findNode(document.graph.root, origin.node)!;
      const writerScopes = loopScopes(document, writer, loop.name);
      if (
        !writerScopes ||
        (donor.name !== writer.name && !beforeInSequence(document, donor, writer)) ||
        (consumer.name !== writer.name && !beforeInSequence(document, consumer, writer))
      )
        continue;
      const writes = writers.flatMap((node) =>
        list(node.writeBindings).filter((write) => overlaps(write?.target, path))
      );
      if (
        writes.length !== 1 ||
        !samePath(writes[0].target, path) ||
        [...donorScopes, ...ancestry, ...writerScopes].some(
          (scope) => JSON.stringify(requiredType(scope.state, path)) !== JSON.stringify(field.type)
        )
      )
        continue;
      const source: NodeDataSource = { kind: 'loop_input', node: donor.name, path: [field.name] };
      if (!choices.has(bindingKey(binding.value)))
        choices.set(bindingKey(binding.value), {
          id: JSON.stringify(source),
          source,
          label: `${origin.node} · ${pathLabel(origin.path)} (previous attempt)`,
          type: field.type,
        });
    }
  }
  return [...choices.values()];
}

export function nodeInputRows(node: GraphNode) {
  return Object.entries(recordFields(node.kind === 'succeed' ? node.output : node.input) ?? {}).map(
    ([name, field]) => ({ name, ...field })
  );
}

export function renameInputField(
  document: Document,
  nodeName: string,
  before: string,
  after: string
): Document {
  const node = findNode(document.graph.root, nodeName)!;
  const schema = node.kind === 'succeed' ? 'output' : 'input';
  const mappings = node.kind === 'succeed' ? 'bindings' : 'inputBindings';
  if (node[mappings] !== undefined && !Array.isArray(node[mappings]))
    throw new Error('Edit this input with its JSON control.');
  return replaceNode(document, nodeName, {
    ...node,
    [schema]: renameField(node[schema], before, after),
    [mappings]: list(node[mappings]).map((binding) =>
      binding?.target?.[0] === before
        ? { ...binding, target: [after, ...binding.target.slice(1)] }
        : binding
    ),
  });
}

export function inputSourceLabel(document: Document, node: GraphNode, name: string): string {
  const mappings = list(node.kind === 'succeed' ? node.bindings : node.inputBindings);
  const index = mappings.findIndex((binding) => samePath(binding?.target, [name]));
  if (index < 0)
    return mappings.some((binding) => binding?.target?.[0] === name)
      ? 'Multiple fields'
      : 'Choose source';
  const binding = mappings[index];
  if (binding?.value?.source === 'state' && Array.isArray(binding.value.path)) {
    const origins = inputOrigins(document, node, binding.value.path);
    if (origins.length) {
      const commonChoice = allNodes(document.graph.root).find(
        (candidate) =>
          candidate.kind === 'choice' &&
          beforeInSequence(document, candidate, node) &&
          origins.every(
            (origin) =>
              !origin.previous &&
              origin.scopes.includes(candidate.name) &&
              samePath(origin.path, origins[0].path)
          ) &&
          choiceOutputFields(candidate).some((field) => field.name === origins[0].path[0])
      );
      if (commonChoice) return `${commonChoice.name} · ${pathLabel(origins[0].path)}`;
      return origins
        .map(
          (origin) =>
            `${origin.node} · ${pathLabel(origin.path)}${origin.previous ? ' (previous attempt)' : ''}`
        )
        .join(' / ');
    }
  }
  const connection = existingDataConnections(document, node)[index];
  if (!connection) return 'Saved source';
  if (connection.producers.length)
    return connection.producers
      .map((source) =>
        source.label
          .replace(' · out · ', ' · ')
          .replace(' · diagnostic · ', ' · ')
          .replace(' · signal · ', ' · ')
      )
      .join(' / ');
  return connection.source
    .replace(/^State · /, 'Current value · ')
    .replace(/^Map item · /, 'Item · ')
    .replace(/^Unresolved source$/, 'Saved source');
}

export function inputSourceId(
  document: Document,
  node: GraphNode,
  name: string,
  choices: NodeDataChoice[]
): string {
  const mappings = list(node.kind === 'succeed' ? node.bindings : node.inputBindings);
  const binding = mappings.find((entry) => samePath(entry?.target, [name]));
  if (!binding) return '';
  const label = inputSourceLabel(document, node, name);
  const matched = choices.filter((choice) => choice.label === label);
  if (matched.length === 1) return matched[0].id;
  if (binding.value?.source === 'state') {
    const origins = inputOrigins(document, node, binding.value.path);
    if (origins.length === 1 && !origins[0].previous) {
      const origin = origins[0];
      const source = choices.find(
        (choice) =>
          choice.source.kind === 'node_output' &&
          choice.source.node === origin.node &&
          choice.source.channel === origin.channel &&
          samePath(choice.source.path, origin.path)
      );
      if (source) return source.id;
    }
  }
  if (binding.value?.source === 'item') {
    const match = choices.find(
      (choice) =>
        choice.source.kind === 'map_item' && samePath(choice.source.path, binding.value.path)
    );
    if (match) return match.id;
  }
  return `saved:${bindingKey(binding.value)}`;
}

export function mapSourceLabel(document: Document, node: GraphNode): string {
  if (node.over?.source === 'item') return `Item · ${pathLabel(node.over.path)}`;
  if (node.over?.source !== 'state' || payloadAtPath(node.state, node.over.path)?.kind !== 'array')
    return 'Choose collection';
  const synthetic: GraphNode = {
    ...node,
    kind: 'step',
    input: {
      kind: 'record',
      fields: { collection: { required: true, type: { kind: 'array', items: { kind: 'null' } } } },
    },
    inputBindings: [{ target: ['collection'], value: node.over }],
  };
  return inputSourceLabel(document, synthetic, 'collection');
}

export function uniqueDataField(fields: { name: string }[], stem = 'field'): string {
  const names = new Set(fields.map((field) => field.name));
  let name = stem;
  for (let suffix = 2; names.has(name); suffix++) name = `${stem}_${suffix}`;
  return name;
}

/** A source-field rename changes only exact output selectors and signal conditions. */
function renameOutputReferences(
  value: any,
  node: string,
  channel: OutputChannel,
  before: string,
  after: string
): void {
  if (!value || typeof value !== 'object') return;
  if (
    value.node === node &&
    value.channel === channel &&
    Array.isArray(value.path) &&
    value.path[0] === before
  )
    value.path = [after, ...value.path.slice(1)];
  if (
    channel === 'signal' &&
    value.name === node &&
    value.source === 'signal' &&
    value.field === before
  )
    value.field = after;
  Object.values(value).forEach((child) =>
    renameOutputReferences(child, node, channel, before, after)
  );
}

function editSignalOutput(node: GraphNode, next: NodeOutputField, previousName?: string): void {
  if (node.kind !== 'verifier' || next.type.kind !== 'enum')
    throw new Error('Choose an outcome field.');
  if (
    node.signals !== undefined &&
    (!node.signals || typeof node.signals !== 'object' || Array.isArray(node.signals))
  )
    throw new Error('Edit this output with its JSON control.');
  const signals =
    node.signals && typeof node.signals === 'object' && !Array.isArray(node.signals)
      ? node.signals
      : {};
  if (next.name !== previousName && Object.hasOwn(signals, next.name))
    throw new Error('This field already exists.');
  node.signals = Object.fromEntries(
    Object.entries(signals).filter(([name]) => name !== previousName)
  );
  node.signals[next.name] = clone(next.type.values);
}

function editRecordOutput(
  node: GraphNode,
  field: NodeOutputField | undefined,
  next: NodeOutputField
): void {
  const previousName = field?.name;
  const key = next.channel === 'diagnostic' ? 'diagnostic' : 'output';
  let schema = node[key];
  if (!recordFields(schema)) {
    if (schema?.kind !== 'null' && schema !== undefined)
      throw new Error('Edit this output with its JSON control.');
    schema = { kind: 'record', fields: {} };
  }
  if (field && next.name !== previousName) schema = renameField(schema, previousName!, next.name);
  else if (!field && Object.hasOwn(recordFields(schema)!, next.name))
    throw new Error('This field already exists.');
  node[key] = {
    ...schema,
    fields: {
      ...recordFields(schema),
      [next.name]: {
        ...(recordFields(schema)?.[next.name] ?? {}),
        type: clone(next.type),
        required: next.required,
      },
    },
  };
}

export function editOutputField(
  document: Document,
  nodeName: string,
  field: NodeOutputField | undefined,
  next: NodeOutputField
): Document {
  const result = clone(document);
  const node = findNode(result.graph.root, nodeName)!;
  if (!/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(next.name) || next.name.length > 128)
    throw new Error('Enter a valid field name.');
  const previousName = field?.name;
  if (field && field.channel !== next.channel) throw new Error('Choose a new output field.');
  if (next.channel === 'signal') editSignalOutput(node, next, previousName);
  else editRecordOutput(node, field, next);
  if (previousName && previousName !== next.name)
    renameOutputReferences(result.graph.root, nodeName, next.channel, previousName, next.name);
  if (field && JSON.stringify(field.type) !== JSON.stringify(next.type))
    synchronizeOutputRoutes(document, result, nodeName, field, next.type);
  return result;
}

export function removeOutputField(
  document: Document,
  nodeName: string,
  field: NodeOutputField
): Document {
  const node = findNode(document.graph.root, nodeName)!;
  const result =
    field.channel === 'signal'
      ? replaceNode(document, nodeName, {
          ...node,
          signals: Object.fromEntries(
            Object.entries(node.signals).filter(([name]) => name !== field.name)
          ),
        })
      : replaceNode(document, nodeName, {
          ...node,
          [field.channel === 'diagnostic' ? 'diagnostic' : 'output']: removeField(
            node[field.channel === 'diagnostic' ? 'diagnostic' : 'output'],
            field.name
          ),
        });
  synchronizeOutputRoutes(document, result, nodeName, field);
  return result;
}

function changedType(current: any, before: Payload, after: Payload): Payload | undefined {
  if (JSON.stringify(current) === JSON.stringify(before)) return clone(after);
  if (current?.kind === 'array') {
    const items = changedType(current.items, before, after);
    if (items) return { ...current, items };
  }
}

function updateFieldType(schema: any, path: string[], before: Payload, after: Payload): void {
  if (!path.length) return;
  let value = schema;
  for (const part of path.slice(0, -1)) {
    const fields = recordFields(value);
    if (!fields?.[part]) return;
    value = fields[part].type;
  }
  const field = recordFields(value)?.[path.at(-1)!];
  const type = field && changedType(field.type, before, after);
  if (type) field.type = type;
}

function writeScopes(document: Document, writer: GraphNode, path: string[]): string[] {
  const scopes: string[] = [];
  for (const owner of pathTo(document.graph.root, writer.name).slice(0, -1).reverse()) {
    if (!isGroup(owner)) continue;
    scopes.push(owner.name);
    if (!promotes(owner, path)) break;
  }
  return scopes;
}

/** Generated slots remain part of later group inputs even when no node reads them. */
function carriedOutputScopes(
  document: Document,
  writer: GraphNode,
  path: string[],
  matches: (binding: any) => boolean
): string[] {
  // Native data authoring reserves a fresh whole-graph name for every inferred route.
  // Authored same-name fields do not establish that ownership relationship.
  if (path.length !== 1 || !/^__ui_data_[1-9][0-9]*$/.test(path[0])) return [];
  const conflicting = new Set<string>();
  for (const other of allNodes(document.graph.root).filter(executable))
    if (
      list(other.writeBindings).some(
        (binding) => !matches(binding) && overlaps(binding?.target, path)
      )
    )
      writeScopes(document, other, path).forEach((scope) => conflicting.add(scope));
  const scopes = new Set<string>();
  for (const group of allNodes(document.graph.root).filter(isGroup)) {
    if (!payloadAtPath(group.state, path)) continue;
    const route = producerRoute(document, writer, group, path);
    if (!route || route.previous) continue;
    const carried = [...route.scopes, group.name];
    if (carried.some((scope) => conflicting.has(scope))) continue;
    carried.forEach((scope) => scopes.add(scope));
  }
  return [...scopes];
}

/** Map item bindings inherit their source type without exposing the collection's storage path. */
function synchronizeMapItems(original: Document, result: Document): void {
  for (const node of allNodes(original.graph.root)) {
    const next = findNode(result.graph.root, node.name)!;
    const beforeItems = bindingContext(original, node).item;
    const afterItems = bindingContext(result, next).item;
    if (JSON.stringify(beforeItems) === JSON.stringify(afterItems)) continue;
    if (node.kind === 'map' && node.over?.source === 'item') {
      const before = payloadAtPath(beforeItems, node.over.path);
      const after = payloadAtPath(afterItems, node.over.path);
      if (before?.kind === 'array' && after?.kind !== 'array') next.over = null;
    }
    if (!executable(node) && node.kind !== 'succeed') continue;
    const key = node.kind === 'succeed' ? 'bindings' : 'inputBindings';
    const schema = node.kind === 'succeed' ? 'output' : 'input';
    for (const binding of list(node[key])) {
      if (binding?.value?.source !== 'item') continue;
      const before = payloadAtPath(beforeItems, binding.value.path);
      const after = payloadAtPath(afterItems, binding.value.path);
      if (!before) continue;
      if (!after) {
        next[key] = list(next[key]).filter((entry) => !samePath(entry.target, binding.target));
      } else {
        next[schema] = clone(next[schema]);
        updateFieldType(next[schema], binding.target, before, after);
      }
    }
  }
}

function hasCompetingWrite(
  original: Document,
  write: any,
  scopes: Set<string>,
  matches: (binding: any) => boolean
): boolean {
  return allNodes(original.graph.root)
    .filter(executable)
    .some((other) =>
      list(other.writeBindings).some(
        (binding) =>
          !matches(binding) &&
          overlaps(binding?.target, write.target) &&
          writeScopes(original, other, write.target).some((scope) => scopes.has(scope))
      )
    );
}

type ConsumerRoute = {
  original: Document;
  result: Document;
  nodeName: string;
  field: NodeOutputField;
  write: any;
  before: Payload;
  after: Payload | undefined;
  scopes: Set<string>;
  competing: boolean;
  callerOwned: boolean;
};

function synchronizeConsumers({
  original,
  result,
  nodeName,
  field,
  write,
  before,
  after,
  scopes,
  competing,
  callerOwned,
}: ConsumerRoute): void {
  for (const consumer of allNodes(original.graph.root)) {
    if (!executable(consumer) && consumer.kind !== 'succeed' && consumer.kind !== 'map') continue;
    const map = consumer.kind === 'map';
    const key = consumer.kind === 'succeed' ? 'bindings' : 'inputBindings';
    const targetSchema = consumer.kind === 'succeed' ? 'output' : 'input';
    const nextConsumer = findNode(result.graph.root, consumer.name)!;
    const bindings = map ? [{ target: [], value: consumer.over }] : list(consumer[key]);
    for (const binding of bindings) {
      if (binding?.value?.source !== 'state' || !prefix(write.target, binding.value.path))
        continue;
      const origins = inputOrigins(original, consumer, binding.value.path);
      if (
        !origins.length ||
        !origins.every(
          (origin) =>
            origin.node === nodeName &&
            origin.channel === field.channel &&
            prefix([field.name], origin.path)
        )
      )
        continue;
      origins.forEach((origin) => origin.scopes.forEach((scope) => scopes.add(scope)));
      if (map) scopes.add(consumer.name);
      updateConsumerBinding({ before, after, write, callerOwned, competing }, {
        binding, map, key, targetSchema, nextConsumer,
      });
    }
  }
}

function updateConsumerBinding(
  route: Pick<ConsumerRoute, 'before' | 'after' | 'write' | 'callerOwned' | 'competing'>,
  consumer: { binding: any; map: boolean; key: string; targetSchema: string; nextConsumer: any }
): void {
  const { before, after, write, callerOwned, competing } = route;
  const { binding, map, key, targetSchema, nextConsumer } = consumer;
  const suffix = binding.value.path.slice(write.target.length);
  const sourceBefore = suffix.length ? payloadAtPath(before, suffix) : before;
  const sourceAfter = after && (suffix.length ? payloadAtPath(after, suffix) : after);
  if (!sourceAfter) {
    if (map) nextConsumer.over = null;
    else
      nextConsumer[key] = list(nextConsumer[key]).filter(
        (entry) => !samePath(entry.target, binding.target)
      );
    return;
  }
  if (callerOwned || competing || !after) return;
  if (map) {
    if (sourceAfter.kind !== 'array') nextConsumer.over = null;
    return;
  }
  if (sourceBefore && sourceAfter) {
    nextConsumer[targetSchema] = clone(nextConsumer[targetSchema]);
    updateFieldType(nextConsumer[targetSchema], binding.target, sourceBefore, sourceAfter);
  }
}

function synchronizeOutputScopes({
  original,
  result,
  writer,
  write,
  before,
  after,
  scopes,
  matches,
}: {
  original: Document;
  result: Document;
  writer: GraphNode;
  write: any;
  before: Payload;
  after: Payload | undefined;
  scopes: Set<string>;
  matches: (binding: any) => boolean;
}): void {
  if (after)
    carriedOutputScopes(original, writer, write.target, matches).forEach((scope) =>
      scopes.add(scope)
    );
  for (const name of scopes) {
    const scope = findNode(result.graph.root, name)!;
    if (before && after) {
      scope.state = clone(scope.state);
      updateFieldType(scope.state, write.target, before, after);
    }
    if (!after && Array.isArray(scope.promotedStatePaths))
      scope.promotedStatePaths = scope.promotedStatePaths.filter(
        (path: unknown) => !samePath(path, write.target)
      );
  }
}

/** Follow exact existing routes; unrelated same-name fields and competing sources stay intact. */
function synchronizeOutputRoutes(
  original: Document,
  result: Document,
  nodeName: string,
  field: NodeOutputField,
  type?: Payload
): void {
  const writer = findNode(original.graph.root, nodeName)!;
  const matches = (binding: any) =>
    binding?.value?.node === nodeName &&
    binding.value.channel === field.channel &&
    prefix([field.name], binding.value.path);
  const writes = list(writer.writeBindings).filter(matches);
  const removedWrites = new Set<any>();
  for (const write of writes) {
    if (!Array.isArray(write.target)) continue;
    const before =
      write.value.path.length === 1
        ? field.type
        : payloadAtPath(field.type, write.value.path.slice(1));
    const after =
      type &&
      (write.value.path.length === 1 ? type : payloadAtPath(type, write.value.path.slice(1)));
    if (!before) continue;
    if (!after) removedWrites.add(write);
    const scopes = new Set(writeScopes(original, writer, write.target));
    const competing = hasCompetingWrite(original, write, scopes, matches);
    const callerOwned = !!payloadAtPath(original.graph.initialInput, write.target);
    synchronizeConsumers({
      original,
      result,
      nodeName,
      field,
      write,
      before,
      after,
      scopes,
      competing,
      callerOwned,
    });
    if (competing || callerOwned) continue;
    synchronizeOutputScopes({ original, result, writer, write, before, after, scopes, matches });
  }
  if (Array.isArray(writer.writeBindings))
    findNode(result.graph.root, nodeName)!.writeBindings = list(
      findNode(result.graph.root, nodeName)!.writeBindings
    ).filter((_, index) => {
      const binding = writer.writeBindings[index];
      return !removedWrites.has(binding) && (type || !matches(binding));
    });
  synchronizeMapItems(original, result);
}

export function finalResultNode(document: Document): GraphNode | undefined {
  const finals = analyzeWorkflowOutcomes(document).completions.filter(
    (entry) => entry.mode === 'final' && entry.boundary === 'run'
  );
  return finals.length === 1 ? findNode(document.graph.root, finals[0].name) : undefined;
}
