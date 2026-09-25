// Authoring routes only. Native validation proves result availability and complete execution paths.
import {
  allNodes,
  children,
  clone,
  executable,
  findNode,
  isGroup,
  pathTo,
  type Document,
  type GraphNode,
} from './domain';
import {
  bindingKey,
  fieldChoices,
  outputChoices,
  pathLabel,
  payloadAtPath,
  selectorLabel,
  type FieldChoice,
} from './bindings';
import { declaredSchemaFit } from './promotion-sources';
import { recordFields, typeSummary, type Payload } from './schema';

export type ProducerSelector = {
  node: string;
  channel: 'out' | 'diagnostic' | 'signal';
  path: string[];
};
export type DataConnectionRequest = {
  consumer: string;
  target: string[];
  source: ProducerSelector;
};
export type DataConnectionPreview = {
  request: DataConnectionRequest;
  canConnect: boolean;
  reasons: string[];
  notes: string[];
  statePath: string[];
  scopes: string[];
  promotions: string[];
  reusesState: boolean;
  type?: Payload;
};
export type ProducerChoice = {
  id: string;
  source: ProducerSelector;
  label: string;
  scope: string;
  available: boolean;
  reason: string;
};
export type ExistingDataConnection = {
  target: string;
  source: string;
  producers: { name: string; label: string }[];
  note?: string;
};

const list = (value: unknown): any[] => (Array.isArray(value) ? value : []);
const object = (value: any): boolean =>
  !!value && typeof value === 'object' && !Array.isArray(value);
const validPath = (value: any): value is string[] =>
  Array.isArray(value) && value.length > 0 && value.every((part) => typeof part === 'string');
const prefix = (a: any, b: any): boolean =>
  validPath(a) && validPath(b) && a.length <= b.length && a.every((part, i) => part === b[i]);
const overlaps = (a: any, b: any): boolean => prefix(a, b) || prefix(b, a);
const promotes = (node: GraphNode, path: string[]): boolean =>
  list(node.promotedStatePaths).some((value) => prefix(value, path));
const mappings = (node: GraphNode): any =>
  node.kind === 'succeed' ? node.bindings : node.inputBindings;

export function dataInputFields(node: GraphNode): FieldChoice[] {
  return executable(node)
    ? fieldChoices(node.input)
    : node.kind === 'succeed'
      ? fieldChoices(node.output)
      : [];
}

export function allDataInputsMapped(node: GraphNode): boolean {
  const fields = dataInputFields(node);
  const leaves = fields.filter(
    (field) =>
      !fields.some(
        (other) => other.path.length > field.path.length && prefix(field.path, other.path)
      )
  );
  return (
    leaves.length > 0 &&
    leaves.every((field) =>
      list(mappings(node)).some((binding) => prefix(binding?.target, field.path))
    )
  );
}

function selectedOutput(document: Document, value: any): { type?: Payload; required: boolean } {
  if (!object(value) || typeof value.node !== 'string' || !validPath(value.path))
    return { required: false };
  const node = findNode(document.graph.root, value.node);
  if (!node || !executable(node)) return { required: false };
  if (value.channel === 'signal' && node.kind === 'verifier' && value.path.length === 1) {
    const labels =
      object(node.signals) && Object.hasOwn(node.signals, value.path[0])
        ? node.signals[value.path[0]]
        : undefined;
    return Array.isArray(labels) &&
      labels.length > 0 &&
      labels.every((label) => typeof label === 'string')
      ? { type: { kind: 'enum', values: clone(labels) }, required: true }
      : { required: false };
  }
  const schema =
    value.channel === 'out'
      ? node.output
      : value.channel === 'diagnostic' && node.kind === 'verifier'
        ? node.diagnostic
        : undefined;
  let current = schema;
  let required = true;
  for (const segment of value.path) {
    const fields = recordFields(current);
    if (!fields || !Object.hasOwn(fields, segment)) return { required: false };
    required &&= fields[segment]?.required === true;
    current = fields[segment]?.type;
  }
  return { type: payloadAtPath(schema, value.path), required };
}

function routeStructure(document: Document, producer: string, consumer: string) {
  const a = pathTo(document.graph.root, producer);
  const b = pathTo(document.graph.root, consumer);
  const reasons: string[] = [];
  const notes: string[] = [];
  let shared = 0;
  while (shared < a.length && shared < b.length && a[shared].name === b[shared].name) shared++;
  const common = a[shared - 1];
  const producerScopes = a.slice(shared, -1).filter(isGroup);
  const consumerScopes = b.slice(shared, -1).filter(isGroup);
  if (!a.length || !b.length || producer === consumer)
    reasons.push('Choose another declared producing node.');
  else if (!common || common.kind !== 'seq')
    reasons.push(
      'The nodes must be in forward branches of a shared Sequence. Parallel peers and alternative branches cannot read each other.'
    );
  else {
    const order = children(common).map((node) => node.name);
    if (order.indexOf(a[shared]?.name) >= order.indexOf(b[shared]?.name))
      reasons.push('The producer must appear before the consumer in their shared Sequence.');
  }
  for (const group of producerScopes) {
    if (group.kind !== 'seq' && !(group.kind === 'par' && group.join?.kind === 'all'))
      reasons.push(
        `Producer crosses ${group.name} (${group.kind}); conditional, repeated, and partial-join results need advanced mappings.`
      );
  }
  for (const group of consumerScopes) {
    if (
      !['seq', 'choice'].includes(group.kind) &&
      !(group.kind === 'par' && group.join?.kind === 'all')
    )
      reasons.push(
        `Consumer crosses ${group.name} (${group.kind}); repeated and partial-join scopes need advanced mappings.`
      );
    if (group.kind === 'choice')
      notes.push(
        `${consumer} runs conditionally inside ${group.name}; the source precedes that choice.`
      );
  }
  if (producerScopes.some((group) => group.kind === 'par'))
    notes.push('The producer returns through a Parallel group that waits for all branches.');
  return { common, producerScopes, consumerScopes, reasons, notes };
}

function authoredRoutes(
  document: Document,
  source: ProducerSelector,
  consumer: string
): string[][] {
  const producer = findNode(document.graph.root, source.node);
  const type = selectedOutput(document, source).type;
  const route = routeStructure(document, source.node, consumer);
  if (!producer || !type || !route.common || route.reasons.length) return [];
  const groups = [route.common, ...route.producerScopes, ...route.consumerScopes];
  const paths = list(producer.writeBindings).flatMap((binding) => {
    if (!validPath(binding?.target) || bindingKey(binding?.value) !== bindingKey(source)) return [];
    if (!route.producerScopes.every((group) => promotes(group, binding.target))) return [];
    if (
      !groups.every(
        (group) => declaredSchemaFit(type, payloadAtPath(group.state, binding.target)) === 'match'
      )
    )
      return [];
    // A competing write means the selected producer is not an unambiguous source for this path.
    if (
      allNodes(document.graph.root)
        .filter(executable)
        .some((writer) =>
          list(writer.writeBindings).some(
            (other) =>
              overlaps(other?.target, binding.target) &&
              (writer.name !== source.node || bindingKey(other?.value) !== bindingKey(source))
          )
        )
    )
      return [];
    return [binding.target];
  });
  return [...new Map(paths.map((path) => [bindingKey(path), path])).values()];
}

const parallelRouteReason =
  'A fresh field cannot return from a parallel producer before error paths are handled inside its branch. Use an existing producer route or advanced mappings and validate the branch first.';

export function dataProducerChoices(document: Document, consumer: GraphNode): ProducerChoice[] {
  return allNodes(document.graph.root)
    .filter(executable)
    .flatMap((node) => {
      const route = routeStructure(document, node.name, consumer.name);
      const scope = pathTo(document.graph.root, node.name)
        .slice(0, -1)
        .map((group) => group.name)
        .join(' / ');
      return outputChoices(node).flatMap(({ value, label }) => {
        const selected = selectedOutput(document, value);
        if (!selected.type) return [];
        const paths = authoredRoutes(document, value, consumer.name);
        const parallelUnavailable =
          route.producerScopes.some((group) => group.kind === 'par') && paths.length !== 1;
        const reason =
          route.reasons[0] ??
          (parallelUnavailable ? parallelRouteReason : undefined) ??
          (!selected.required
            ? 'Optional output field; use advanced mappings and Validate its presence.'
            : 'Earlier in the shared Sequence; result availability still needs validation.');
        return [
          {
            id: bindingKey(value),
            source: value,
            label: `${node.name} · ${label}`,
            scope,
            available: route.reasons.length === 0 && selected.required && !parallelUnavailable,
            reason,
          },
        ];
      });
    });
}

function freshStatePath(document: Document, source: ProducerSelector, consumer: string): string[] {
  // Reserve every field/path in the document, including dangling mappings and future metadata.
  // An old reference must never become attached to a newly generated route by name reuse.
  const reserved = new Set<string>();
  const pending: any[] = [document.graph];
  while (pending.length) {
    const value = pending.pop();
    if (!value || typeof value !== 'object') continue;
    for (const [key, item] of Object.entries(value)) {
      reserved.add(key);
      if (typeof item === 'string') reserved.add(item);
      else if (item && typeof item === 'object') pending.push(item);
    }
  }
  const stem = `from_${source.node}_${source.channel}_${source.path.join('_')}_to_${consumer}`
    .replace(/[^A-Za-z0-9_]/g, '_')
    .slice(0, 100);
  let name = stem;
  for (let i = 2; reserved.has(name); i++) name = `${stem}_${i}`;
  return [name];
}

function dataFieldReasons(
  request: DataConnectionRequest,
  consumer: GraphNode | undefined,
  producer: GraphNode | undefined,
  selected: ReturnType<typeof selectedOutput>,
  target: ReturnType<typeof dataInputFields>[number] | undefined
): string[] {
  const reasons: string[] = [];
  if (!consumer || (!executable(consumer) && consumer.kind !== 'succeed'))
    reasons.push('Select an Agent, Verifier, or Success consumer.');
  if (!target) reasons.push('The target field is not declared in this consumer schema.');
  if (!selected.type)
    reasons.push('The producer field is unavailable or uses an unsupported schema.');
  else if (!selected.required)
    reasons.push('The output field is optional. Use advanced mappings to handle its absence.');
  if (selected.type && target && declaredSchemaFit(selected.type, target.type) !== 'match')
    reasons.push(
      'The declared field types do not have a confirmed match. Use advanced mappings and Validate.'
    );
  if (consumer) {
    const current = mappings(consumer);
    if (current !== undefined && !Array.isArray(current))
      reasons.push(
        'The existing consumer mappings use an unsupported format; preserve and edit them in advanced mappings.'
      );
    if (list(current).some((binding) => !validPath(binding?.target)))
      reasons.push('An unresolved target mapping prevents safely adding another one.');
    if (list(current).some((binding) => overlaps(binding?.target, request.target)))
      reasons.push(
        'This target already has a mapping. Edit or remove that mapping in advanced mappings before connecting another producer.'
      );
  }
  if (producer && producer.writeBindings !== undefined && !Array.isArray(producer.writeBindings))
    reasons.push('The producer has an unsupported write format; use advanced mappings.');
  return reasons;
}

function scopeReasons(
  route: ReturnType<typeof routeStructure>,
  groups: GraphNode[],
  reused: string[] | undefined
): string[] {
  const reasons: string[] = [];
  if (!reused && route.producerScopes.some((group) => group.kind === 'par'))
    reasons.push(parallelRouteReason);
  for (const group of groups) {
    if (!recordFields(group.state))
      reasons.push(`${group.name} needs Object state before it can carry a named data connection.`);
    if (
      route.producerScopes.includes(group) &&
      group.promotedStatePaths !== undefined &&
      !Array.isArray(group.promotedStatePaths)
    )
      reasons.push(`${group.name} has an unsupported promotion format; use advanced mappings.`);
  }
  return reasons;
}

export function previewDataConnection(
  document: Document,
  request: DataConnectionRequest
): DataConnectionPreview {
  const consumer = findNode(document.graph.root, request.consumer);
  const producer =
    typeof request.source?.node === 'string'
      ? findNode(document.graph.root, request.source.node)
      : undefined;
  const selected = selectedOutput(document, request.source);
  const route = routeStructure(document, producer?.name ?? '', consumer?.name ?? '');
  const target =
    consumer &&
    dataInputFields(consumer).find(
      (field) => bindingKey(field.path) === bindingKey(request.target)
    );
  const reasons = [
    ...route.reasons,
    ...dataFieldReasons(request, consumer, producer, selected, target || undefined),
  ];
  const groups = [route.common, ...route.producerScopes, ...route.consumerScopes].filter(
    (node): node is GraphNode => !!node && isGroup(node)
  );
  const authored =
    producer && selected.type ? authoredRoutes(document, request.source, request.consumer) : [];
  const reused = authored.length === 1 ? authored[0] : undefined;
  if (authored.length > 1)
    reasons.push(
      'Multiple authored routes carry this producer. Choose the intended state path in advanced mappings.'
    );
  reasons.push(...scopeReasons(route, groups, reused));
  return {
    request: clone(request),
    canConnect: reasons.length === 0,
    reasons: [...new Set(reasons)],
    notes: [
      ...route.notes,
      'Rust validation must prove that the producer returns a value on every path that reaches this consumer, including error and guard outcomes.',
    ],
    statePath: reused
      ? clone(reused)
      : producer && selected.type
        ? freshStatePath(document, request.source, request.consumer)
        : [],
    scopes: reused ? [] : [...new Set(groups.map((group) => group.name))],
    promotions: reused ? [] : [...route.producerScopes].reverse().map((group) => group.name),
    reusesState: !!reused,
    type: selected.type,
  };
}

export function connectData(document: Document, request: DataConnectionRequest): Document {
  const plan = previewDataConnection(document, request);
  if (!plan.canConnect || !plan.type)
    throw new Error(plan.reasons[0] ?? 'This route is unavailable.');
  const next = clone(document);
  const [field] = plan.statePath;
  for (const name of plan.scopes) {
    const group = findNode(next.graph.root, name)!;
    group.state = {
      ...group.state,
      fields: { ...group.state.fields, [field]: { type: clone(plan.type), required: false } },
    };
  }
  const producer = findNode(next.graph.root, request.source.node)!;
  if (!plan.reusesState)
    producer.writeBindings = [
      ...list(producer.writeBindings),
      { target: clone(plan.statePath), value: clone(request.source) },
    ];
  for (const name of plan.promotions) {
    const group = findNode(next.graph.root, name)!;
    group.promotedStatePaths = [...list(group.promotedStatePaths), clone(plan.statePath)];
  }
  const consumer = findNode(next.graph.root, request.consumer)!;
  const key = consumer.kind === 'succeed' ? 'bindings' : 'inputBindings';
  consumer[key] = [
    ...list(consumer[key]),
    { target: clone(request.target), value: { source: 'state', path: clone(plan.statePath) } },
  ];
  return next;
}

function existingProducers(document: Document, consumer: GraphNode, value: any) {
  if (!object(value) || value.source !== 'state' || !validPath(value.path)) return [];
  const sources: { name: string; label: string }[] = [];
  for (const writer of allNodes(document.graph.root).filter(executable)) {
    const route = routeStructure(document, writer.name, consumer.name);
    if (!route.common || route.common.kind !== 'seq' || route.reasons.length) continue;
    if (!route.producerScopes.every((group) => promotes(group, value.path))) continue;
    if (
      ![route.common, ...route.producerScopes, ...route.consumerScopes].every((group) =>
        payloadAtPath(group.state, value.path)
      )
    )
      continue;
    for (const binding of list(writer.writeBindings)) {
      if (!prefix(binding?.target, value.path) || !validPath(binding?.value?.path)) continue;
      const selector = {
        ...binding.value,
        path: [...binding.value.path, ...value.path.slice(binding.target.length)],
      };
      if (!selectedOutput(document, selector).type) continue;
      sources.push({ name: selector.node, label: selectorLabel(selector) });
    }
  }
  return [...new Map(sources.map((source) => [source.label, source])).values()];
}

function couldOverwriteBefore(
  document: Document,
  writer: GraphNode,
  consumer: GraphNode,
  path: string[]
): boolean {
  const a = pathTo(document.graph.root, writer.name);
  const b = pathTo(document.graph.root, consumer.name);
  let shared = 0;
  while (shared < a.length && shared < b.length && a[shared].name === b[shared].name) shared++;
  // A later write may feed a subsequent visit inside a repeated scope. Keep its origin unresolved.
  if (a.slice(0, shared).some((node) => node.kind === 'loop' || node.kind === 'map')) return true;
  if (writer.name === consumer.name) return false;
  const common = a[shared - 1];
  if (!common) return true;
  if (common.kind === 'seq') {
    const order = children(common).map((node) => node.name);
    if (order.indexOf(a[shared]?.name) >= order.indexOf(b[shared]?.name)) return false;
  } else if (common.kind === 'choice' || common.kind === 'par') {
    // Alternative/parallel branches receive their own incoming state; peer writes do not enter it.
    return false;
  } else return true;
  return a
    .slice(shared, -1)
    .filter(isGroup)
    .every((group) => {
      const promotions = group.promotedStatePaths;
      if (promotions === undefined) return false;
      if (!Array.isArray(promotions) || promotions.some((entry) => !validPath(entry))) return true;
      return promotes(group, path);
    });
}

function isUntouchedRunInput(document: Document, consumer: GraphNode, value: any): boolean {
  if (!object(value) || value.source !== 'state' || !validPath(value.path)) return false;
  let initial = document.graph.initialInput;
  for (const part of value.path) {
    const fields = recordFields(initial);
    if (!fields || !Object.hasOwn(fields, part) || fields[part]?.required !== true) return false;
    initial = fields[part].type;
  }
  if (!initial || typeof initial.kind !== 'string') return false;
  const scopes = pathTo(document.graph.root, consumer.name).slice(0, -1).filter(isGroup);
  if (
    scopes.some(
      (group) =>
        group.kind === 'map' ||
        declaredSchemaFit(initial, payloadAtPath(group.state, value.path)) !== 'match'
    )
  )
    return false;
  return !allNodes(document.graph.root)
    .filter(executable)
    .some((writer) => {
      const writes = writer.writeBindings;
      const mayWrite =
        writes !== undefined &&
        (!Array.isArray(writes) ||
          writes.some(
            (binding) => !validPath(binding?.target) || overlaps(binding.target, value.path)
          ));
      return mayWrite && couldOverwriteBefore(document, writer, consumer, value.path);
    });
}

export function existingDataConnections(
  document: Document,
  node: GraphNode
): ExistingDataConnection[] {
  const current = mappings(node);
  if (current !== undefined && !Array.isArray(current))
    return [
      {
        target: 'Unresolved mappings',
        source: 'Advanced format preserved',
        producers: [],
        note: 'Use advanced mappings to inspect this value.',
      },
    ];
  return list(current).map((binding) => {
    const producers = existingProducers(document, node, binding?.value);
    const runInput = producers.length === 0 && isUntouchedRunInput(document, node, binding?.value);
    return {
      target: pathLabel(binding?.target),
      source: runInput
        ? `Run input · ${pathLabel(binding.value.path)}`
        : selectorLabel(binding?.value),
      producers,
      note: runInput
        ? 'Supplied when the run starts and passed unchanged to this node.'
        : producers.length > 1
          ? 'Multiple possible writers. Rust validation determines availability.'
          : producers.length
            ? 'Declared route through state and group returns; Rust checks execution paths.'
            : 'Incoming, conditional, or unresolved source. Existing mapping is preserved.',
    };
  });
}

export const connectionTypeLabel = (plan: DataConnectionPreview): string => typeSummary(plan.type);
