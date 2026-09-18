// Field choices for the Rust binding wire types. These helpers inspect schemas;
// Rust still owns compatibility, availability, and graph admission.
import { isGroup, pathTo, type Document, type GraphNode } from './domain';
import { recordFields, typeSummary, type Payload } from './schema';

export type FieldChoice = { path: string[]; type: Payload; label: string };
export type BindingChoice = { value: any; label: string };
export type BindingContext = { state: any; item: any; stateName: string; itemName?: string };

export function pathLabel(path: unknown): string {
  if (!Array.isArray(path) || !path.length || !path.every((segment) => typeof segment === 'string'))
    return 'Unresolved field';
  return path.join(' › ');
}

// Array segments are encoded separately: ["a.b"] never becomes ["a", "b"].
export function bindingKey(value: any): string {
  if (Array.isArray(value)) return JSON.stringify(['path', value]);
  if (value && typeof value === 'object') {
    if ('source' in value) return JSON.stringify(['data', value.source, value.path]);
    if ('node' in value) return JSON.stringify(['output', value.node, value.channel, value.path]);
  }
  return JSON.stringify(['unresolved', value]) ?? 'unresolved';
}

export function payloadAtPath(payload: any, path: unknown): Payload | undefined {
  if (!Array.isArray(path) || !path.length || !path.every((part) => typeof part === 'string'))
    return undefined;
  let current = payload;
  for (const part of path) {
    const fields = recordFields(current);
    if (!fields || !Object.hasOwn(fields, part)) return undefined;
    current = fields[part]?.type;
  }
  return current && typeof current.kind === 'string' ? current : undefined;
}

export function fieldChoices(payload: any): FieldChoice[] {
  const result: FieldChoice[] = [];
  function visit(value: any, prefix: string[], ancestors: Set<unknown>) {
    if (prefix.length >= 64 || ancestors.has(value)) return;
    const fields = recordFields(value);
    if (!fields) return;
    const nextAncestors = new Set(ancestors).add(value);
    for (const [name, field] of Object.entries(fields)) {
      const type = field?.type;
      if (!type || typeof type.kind !== 'string') continue;
      const path = [...prefix, name];
      result.push({ path, type, label: `${pathLabel(path)} · ${typeSummary(type)}` });
      // FieldPath traverses records, never array indices or dotted-name fragments.
      if (type.kind === 'record') visit(type, path, nextAncestors);
    }
  }
  visit(payload, [], new Set());
  return result;
}

export function bindingContext(document: Document, node: GraphNode): BindingContext {
  let state: any = document.graph.initialInput;
  let item: any;
  let stateName = 'initial input';
  let itemName: string | undefined;
  for (const ancestor of pathTo(document.graph.root, node.name).slice(0, -1)) {
    if (ancestor.kind === 'map') {
      // A map's over selector reads its own state, or the enclosing map item.
      const source =
        ancestor.over?.source === 'item'
          ? item
          : ancestor.over?.source === 'state'
            ? ancestor.state
            : undefined;
      const selected = payloadAtPath(source, ancestor.over?.path);
      item = selected?.kind === 'array' ? selected.items : undefined;
      itemName = ancestor.name;
    }
    if (isGroup(ancestor)) {
      state = ancestor.state;
      stateName = ancestor.name;
    }
  }
  return { state, item, stateName, itemName };
}

export function dataChoices(context: Pick<BindingContext, 'state' | 'item'>): BindingChoice[] {
  return (['state', 'item'] as const).flatMap((source) =>
    fieldChoices(context[source]).map(({ path, label }) => ({
      value: { source, path },
      label: `${source === 'state' ? 'State' : 'Map item'} · ${label}`,
    }))
  );
}

export function outputChoices(node: GraphNode): BindingChoice[] {
  const payloads = [
    { channel: 'out', label: 'Output', payload: node.output },
    ...(node.kind === 'verifier'
      ? [{ channel: 'diagnostic', label: 'Diagnostic', payload: node.diagnostic }]
      : []),
  ];
  const result = payloads.flatMap(({ channel, label: channelLabel, payload }) =>
    fieldChoices(payload).map(({ path, label }) => ({
      value: { node: node.name, channel, path },
      label: `${channelLabel} · ${label}`,
    }))
  );
  if (node.kind === 'verifier' && node.signals && typeof node.signals === 'object') {
    for (const name of Object.keys(node.signals)) {
      result.push({
        value: { node: node.name, channel: 'signal', path: [name] },
        label: `Signal · ${name}`,
      });
    }
  }
  return result;
}

export function selectorLabel(value: any): string {
  if (value?.source === 'state') return `State · ${pathLabel(value.path)}`;
  if (value?.source === 'item') return `Map item · ${pathLabel(value.path)}`;
  if (typeof value?.node === 'string' && typeof value?.channel === 'string')
    return `${value.node} · ${value.channel} · ${pathLabel(value.path)}`;
  return 'Unresolved source';
}

export function pathChoices(payload: any): BindingChoice[] {
  return fieldChoices(payload).map(({ path, label }) => ({ value: path, label }));
}

export function mapOverChoices(node: GraphNode, context: BindingContext): BindingChoice[] {
  const sources = { state: node.state, item: context.item };
  return dataChoices(sources).filter(
    ({ value }) =>
      payloadAtPath(sources[value.source as 'state' | 'item'], value.path)?.kind === 'array'
  );
}

export function promotionChoices(node: GraphNode): BindingChoice[] {
  return fieldChoices(node.state)
    .filter(({ type }) => node.kind !== 'map' || type.kind === 'array')
    .map(({ path, label }) => ({ value: path, label }));
}

export function updateMapping(
  node: GraphNode,
  field: string,
  index: number,
  patch: Record<string, any>
): GraphNode {
  if (!Array.isArray(node[field])) return node;
  const entries = node[field];
  return {
    ...node,
    [field]: entries.map((entry: any, at: number) =>
      at === index ? { ...entry, ...patch } : entry
    ),
  };
}

export function removeMapping(node: GraphNode, field: string, index: number): GraphNode {
  if (!Array.isArray(node[field])) return node;
  return { ...node, [field]: node[field].filter((_: any, at: number) => at !== index) };
}

export function appendMapping(node: GraphNode, field: string, target: any, value: any): GraphNode {
  if (node[field] !== undefined && !Array.isArray(node[field])) return node;
  return { ...node, [field]: [...(node[field] ?? []), { target, value }] };
}

export function togglePromotion(node: GraphNode, path: any, selected: boolean): GraphNode {
  if (node.promotedStatePaths !== undefined && !Array.isArray(node.promotedStatePaths)) return node;
  const paths = Array.isArray(node.promotedStatePaths) ? node.promotedStatePaths : [];
  const matches = (candidate: any) => bindingKey(candidate) === bindingKey(path);
  const next = selected
    ? paths.some(matches)
      ? paths
      : [...paths, path]
    : paths.filter((candidate: any) => !matches(candidate));
  return { ...node, promotedStatePaths: next };
}
