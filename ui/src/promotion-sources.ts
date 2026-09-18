// Structural authoring assistance, not a graph verifier. This follows authored
// writes and promotion boundaries; Rust proves execution/guard availability.
import {
  allNodes,
  clone,
  executable,
  findNode,
  isGroup,
  pathTo,
  type Document,
  type GraphNode,
} from './domain';
import {
  bindingContext,
  bindingKey,
  fieldChoices,
  outputChoices,
  pathLabel,
  payloadAtPath,
  selectorLabel,
  togglePromotion,
} from './bindings';
import { recordFields, typeSummary, type Payload } from './schema';

export type SchemaFit = 'match' | 'mismatch' | 'unknown';
export type PromotionSource = {
  id: string;
  writer: string;
  selector: any;
  label: string;
  existing: boolean;
  chain: string[];
  missing: string[];
  issues: string[];
  notes: string[];
  connected: boolean;
  canConnect: boolean;
};
export type PromotionTrace = {
  path: any;
  label: string;
  selected: boolean;
  parent: string;
  fieldType: string;
  parentType: string;
  schemaNote?: string;
  issues: string[];
  sources: PromotionSource[];
  passThrough: boolean;
  passThroughNote: string;
};

const validPath = (path: any): path is string[] =>
  Array.isArray(path) && path.length > 0 && path.every((part) => typeof part === 'string');
const samePath = (a: any, b: any) => bindingKey(a) === bindingKey(b);
const prefix = (a: any, b: any) =>
  validPath(a) && validPath(b) && a.length <= b.length && a.every((part, i) => part === b[i]);
const overlaps = (a: any, b: any) => prefix(a, b) || prefix(b, a);
const selected = (node: GraphNode, path: any) =>
  Array.isArray(node.promotedStatePaths) &&
  node.promotedStatePaths.some((p: any) => samePath(p, path));
const promotes = (node: GraphNode, path: any) =>
  list(node.promotedStatePaths).some((candidate) => prefix(candidate, path));
const list = (value: any): any[] => (Array.isArray(value) ? value : []);

// Only claim conclusive, simple schema relations. Different record shapes can be
// valid subtypes: leave those to Rust instead of duplicating its type algebra.
export function declaredSchemaFit(source: any, target: any, depth = 0): SchemaFit {
  if (
    !source ||
    !target ||
    typeof source.kind !== 'string' ||
    typeof target.kind !== 'string' ||
    depth > 64
  )
    return 'unknown';
  if (source.kind === 'integer' && target.kind === 'number') return 'match';
  if (source.kind !== target.kind) return 'mismatch';
  if (['null', 'string', 'boolean', 'integer', 'number'].includes(source.kind)) return 'match';
  if (source.kind === 'array') return declaredSchemaFit(source.items, target.items, depth + 1);
  if (source.kind === 'enum') {
    if (!Array.isArray(source.values) || !Array.isArray(target.values)) return 'unknown';
    return source.values.every(
      (value: any) => typeof value === 'string' && target.values.includes(value)
    )
      ? 'match'
      : 'mismatch';
  }
  if (source.kind === 'record') {
    const a = recordFields(source),
      b = recordFields(target);
    if (!a || !b || Object.keys(a).length !== Object.keys(b).length) return 'unknown';
    return Object.keys(a).every(
      (name) =>
        Object.hasOwn(b, name) &&
        a[name]?.required === b[name]?.required &&
        declaredSchemaFit(a[name]?.type, b[name]?.type, depth + 1) === 'match'
    )
      ? 'match'
      : 'unknown';
  }
  return 'unknown';
}

function requiredPath(schema: any, path: any): boolean {
  if (!validPath(path)) return false;
  let value = schema;
  for (const segment of path) {
    const fields = recordFields(value);
    if (!fields || !Object.hasOwn(fields, segment) || fields[segment]?.required !== true)
      return false;
    value = fields[segment].type;
  }
  return true;
}

function outputType(document: Document, selector: any): Payload | undefined {
  if (typeof selector?.node !== 'string' || !validPath(selector?.path)) return undefined;
  const source = findNode(document.graph.root, selector.node);
  if (!source || !executable(source)) return undefined;
  if (selector.channel === 'out') return payloadAtPath(source.output, selector.path);
  if (source.kind === 'verifier' && selector.channel === 'diagnostic')
    return payloadAtPath(source.diagnostic, selector.path);
  if (
    source.kind === 'verifier' &&
    selector.channel === 'signal' &&
    selector.path.length === 1 &&
    source.signals &&
    Object.hasOwn(source.signals, selector.path[0])
  ) {
    const values = source.signals[selector.path[0]];
    return Array.isArray(values) && values.every((v: any) => typeof v === 'string')
      ? { kind: 'enum', values }
      : undefined;
  }
  return undefined;
}

// A promoted map field has an item type inside its body. Nested maps peel one
// array layer each. This is path/type presentation only, never a flow proof.
function enclosingType(
  document: Document,
  node: GraphNode,
  path: any,
  planned = new Set<string>()
) {
  const ancestors = pathTo(document.graph.root, node.name).slice(0, -1);
  let override: Payload | undefined;
  let indexed = false;
  for (const ancestor of ancestors) {
    if (ancestor.kind !== 'map' || (!selected(ancestor, path) && !planned.has(ancestor.name)))
      continue;
    const arrayType = override ?? payloadAtPath(ancestor.state, path);
    if (arrayType?.kind !== 'array') return { type: undefined, indexed: true };
    override = arrayType.items;
    indexed = true;
  }
  return { type: override ?? payloadAtPath(bindingContext(document, node).state, path), indexed };
}

function addFitIssue(issues: string[], source: any, target: any, where: string) {
  if (!target) issues.push(`${where} has no matching state field.`);
  else {
    const fit = declaredSchemaFit(source, target);
    if (fit === 'mismatch') issues.push(`${where} has a different field type.`);
    if (fit === 'unknown')
      issues.push(
        `${where} needs an advanced schema check; configure this route manually and Validate.`
      );
  }
}

function isParallelConflict(
  group: GraphNode,
  first: GraphNode,
  other: GraphNode,
  path: string[],
  planned: Set<string>
): boolean {
  const a = pathTo(group, first.name),
    b = pathTo(group, other.name);
  let i = 0;
  while (i < a.length && i < b.length && a[i].name === b[i].name) i++;
  const common = a[i - 1];
  return (
    !!common &&
    common.kind === 'par' &&
    common.join?.kind === 'all' &&
    a
      .slice(i, -1)
      .filter(isGroup)
      .every((node) => planned.has(node.name) || promotes(node, path)) &&
    b
      .slice(i, -1)
      .filter(isGroup)
      .every((node) => promotes(node, path))
  );
}

function sourceRoute(
  document: Document,
  group: GraphNode,
  path: string[],
  writer: GraphNode,
  selector: any,
  bindingIndex?: number
): PromotionSource {
  const chainNodes = pathTo(group, writer.name).slice(0, -1).filter(isGroup);
  const chain = [...chainNodes].reverse().map((node) => node.name);
  const planned = new Set(chain);
  const missing = [...chainNodes]
    .reverse()
    .filter((node) => !promotes(node, path))
    .map((node) => node.name);
  const issues: string[] = [];
  const notes: string[] = [
    `${writer.kind === 'verifier' ? 'Verifier' : 'Agent'} ${writer.name} can finish with timeout, crash, malformed, or refusal and produce no result. Route those outcomes to Failure or recovery before the group completes. A connected mapping alone does not guarantee a value; Validate checks completion paths.`,
  ];
  const existing = bindingIndex !== undefined;
  let valueType = outputType(document, selector);
  if (!valueType) issues.push('The selected child output field is unavailable.');
  if (selector?.node !== writer.name)
    notes.push('This write reads another node; Validate checks that its output is available.');
  if (
    chainNodes.some(
      (node) => node.kind === 'choice' || (node.kind === 'par' && node.join?.kind !== 'all')
    )
  )
    notes.push('This route crosses conditional branches. Validate checks which sources complete.');
  for (const node of chainNodes) {
    if (!payloadAtPath(node.state, path))
      issues.push(`Add ${pathLabel(path)} to ${node.name} state before connecting.`);
    if (node.promotedStatePaths !== undefined && !Array.isArray(node.promotedStatePaths))
      issues.push(`${node.name} has an unsupported promotion format.`);
  }
  const target = enclosingType(document, writer, path, planned);
  addFitIssue(
    issues,
    valueType,
    target.type,
    `State around ${writer.name}${target.indexed ? ' (per item)' : ''}`
  );
  for (const node of [...chainNodes].reverse()) {
    if (node.kind === 'map') {
      if (payloadAtPath(node.state, path)?.kind !== 'array')
        issues.push(`${node.name} can collect only array fields.`);
      valueType = valueType ? { kind: 'array', items: valueType } : undefined;
    }
    addFitIssue(
      issues,
      valueType,
      enclosingType(document, node, path, planned).type,
      `Parent of ${node.name}`
    );
  }
  if (!existing) {
    if (writer.writeBindings !== undefined && !Array.isArray(writer.writeBindings))
      issues.push(`${writer.name} has an unsupported write format.`);
    if (list(writer.writeBindings).some((binding) => overlaps(binding?.target, path)))
      issues.push(`${writer.name} already writes this state path. Edit that mapping instead.`);
  }
  for (const other of allNodes(document.graph.root).filter(executable)) {
    if (
      other.name !== writer.name &&
      isParallelConflict(document.graph.root, writer, other, path, planned) &&
      list(other.writeBindings).some((binding) => overlaps(binding?.target, path))
    ) {
      issues.push(
        `${other.name} also writes this field in a parallel branch. Resolve the conflict first.`
      );
    }
  }
  return {
    id: JSON.stringify([writer.name, bindingIndex ?? 'new', selector]),
    writer: writer.name,
    selector,
    label: selectorLabel(selector),
    existing,
    chain,
    missing,
    issues: [...new Set(issues)],
    notes,
    connected: existing && missing.length === 0 && issues.length === 0,
    canConnect: issues.length === 0,
  };
}

export function promotionTrace(document: Document, group: GraphNode, path: any): PromotionTrace {
  const context = bindingContext(document, group);
  const ownType = payloadAtPath(group.state, path);
  const parent = enclosingType(document, group, path);
  const issues: string[] = [];
  if (!ownType) issues.push('This path is absent from the group state schema.');
  if (!parent.type)
    issues.push(`Add ${pathLabel(path)} to ${context.stateName} state before returning it.`);
  if (group.kind === 'map' && ownType?.kind !== 'array')
    issues.push('A map collects child values into an array field.');
  const passThrough =
    group.kind !== 'map' &&
    !parent.indexed &&
    requiredPath(group.state, path) &&
    requiredPath(context.state, path) &&
    !!parent.type &&
    declaredSchemaFit(ownType, parent.type) === 'match';
  const sources: PromotionSource[] = [];
  if (validPath(path)) {
    for (const writer of allNodes(group).filter(executable)) {
      list(writer.writeBindings).forEach((binding, index) => {
        if (!prefix(binding?.target, path)) return;
        // Whole-record writes can supply a promoted descendant field too.
        const rest = path.slice(binding.target.length);
        const selector =
          rest.length && validPath(binding?.value?.path)
            ? { ...binding.value, path: [...binding.value.path, ...rest] }
            : binding?.value;
        const route = sourceRoute(document, group, path, writer, selector, index);
        if (rest.length) {
          route.issues.push(
            'This value comes from a whole-record write. Validate its promotion path before changing the route.'
          );
          route.canConnect = false;
          route.connected = false;
        }
        sources.push(route);
      });
      for (const { value } of outputChoices(writer)) {
        if (
          sources.some(
            (source) =>
              source.writer === writer.name && bindingKey(source.selector) === bindingKey(value)
          )
        )
          continue;
        sources.push(sourceRoute(document, group, path, writer, value));
      }
    }
  }
  return {
    path,
    label: pathLabel(path),
    selected: selected(group, path),
    parent: context.stateName,
    fieldType: typeSummary(ownType),
    parentType: typeSummary(parent.type),
    schemaNote:
      ownType && parent.type && declaredSchemaFit(ownType, parent.type) === 'mismatch'
        ? `Group schema: ${typeSummary(ownType)}; parent field accepts: ${typeSummary(parent.type)}.`
        : undefined,
    issues,
    sources,
    passThrough,
    passThroughNote:
      group.kind === 'map' || parent.indexed
        ? 'Collected map fields require a child write for each item; incoming state is not a producer.'
        : passThrough
          ? 'This required incoming field can pass through unchanged. It is not a child-produced result.'
          : 'No confirmed incoming value to pass through. The parent must require this field, or Validate must prove an earlier write supplies it.',
  };
}

export function promotionTraces(document: Document, group: GraphNode): PromotionTrace[] {
  const paths: any[] = fieldChoices(group.state).map(({ path }) => path);
  for (const path of list(group.promotedStatePaths))
    if (!paths.some((known) => samePath(known, path))) paths.push(path);
  return paths.map((path) => promotionTrace(document, group, path));
}

export function connectPromotionSource(
  document: Document,
  group: GraphNode,
  path: any,
  sourceId: string
): GraphNode {
  const source = promotionTrace(document, group, path).sources.find(
    (candidate) => candidate.id === sourceId
  );
  if (!source?.canConnect)
    throw new Error(source?.issues[0] ?? 'This source is no longer available.');
  const next = clone(group);
  const writer = findNode(next, source.writer);
  if (!writer) throw new Error('The producing node is no longer in this group.');
  if (!source.existing)
    writer.writeBindings = [
      ...list(writer.writeBindings),
      { target: clone(path), value: clone(source.selector) },
    ];
  for (const name of source.chain) {
    const wrapper = findNode(next, name);
    if (wrapper && !promotes(wrapper, path))
      Object.assign(wrapper, togglePromotion(wrapper, clone(path), true));
  }
  return next;
}
