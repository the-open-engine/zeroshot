import {
  assertDocument,
  bindingFor,
  clone,
  executable,
  findNode,
  pathTo,
  replaceNode,
  type Document,
  type GraphNode,
} from './domain';
import { bindingContext, bindingKey } from './bindings';
import { recordFields, type Payload } from './schema';

export type ParentStateInput = {
  input: Payload;
  inputBindings: any[];
  sourceName: string;
  optionalFields: number;
  replacesExisting: boolean;
};

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
const record = (value: any) => !!value && typeof value === 'object' && !Array.isArray(value);

function parentFieldBinding(name: string, field: any, maps: GraphNode[], existing: any[]) {
  if (!record(field) || !record(field.type) || typeof field.type.kind !== 'string')
    throw new Error('Repair the unsupported parent state fields before using them as input.');
  if (field.required !== true) return undefined;
  if (!/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(name) || name.length > 128)
    throw new Error(`Parent field ${name} needs a valid field name before it can be connected.`);
  // Native map promotions reinterpret these state paths as per-item writes. A
  // copied array schema cannot safely assume the same read type inside its body.
  if (
    maps.some(
      (map) =>
        !Array.isArray(map.promotedStatePaths ?? []) ||
        (map.promotedStatePaths ?? []).some((path: any) => !Array.isArray(path) || path[0] === name)
    )
  )
    throw new Error(
      `Parent field ${name} participates in map collection. Use explicit input mappings for this activity.`
    );

  // Bind a complete top-level value, including nested records/arrays. Emitting
  // descendant bindings too would create overlapping targets in the native wire format.
  const target = [name];
  const value = { source: 'state', path: [name] };
  const unchanged = existing.filter(
    (binding) =>
      record(binding) &&
      bindingKey(binding.target) === bindingKey(target) &&
      bindingKey(binding.value) === bindingKey(value)
  );
  return unchanged.length === 1 ? clone(unchanged[0]) : { target, value };
}

/** Explicit input replacement only; ordinary schema edits never infer mappings. */
export function parentStateInput(document: Document, node: GraphNode): ParentStateInput {
  if (!executable(node) || bindingFor(document.runtime, node.name)?.kind === 'git_delivery')
    throw new Error('Choose an Agent or Verifier with an editable input contract.');
  const { state, stateName } = bindingContext(document, node);
  const fields = recordFields(state);
  if (!fields && state?.kind !== 'null')
    throw new Error(
      'Parent state must be an object or None. Other types need explicit input mappings.'
    );
  if (node.inputBindings !== undefined && !Array.isArray(node.inputBindings))
    throw new Error('Repair the unsupported input mappings before replacing them.');

  const existing: any[] = node.inputBindings ?? [];
  const maps = pathTo(document.graph.root, node.name)
    .slice(0, -1)
    .filter((parent) => parent.kind === 'map');
  const inputBindings: any[] = [];
  let optionalFields = 0;
  for (const [name, field] of Object.entries(fields ?? {})) {
    const binding = parentFieldBinding(name, field, maps, existing);
    if (!binding) {
      optionalFields++;
      continue;
    }
    inputBindings.push(binding);
  }
  const input = clone(state);
  const emptyInput =
    same(node.input, { kind: 'null' }) || same(node.input, { kind: 'record', fields: {} });
  return {
    input,
    inputBindings,
    sourceName: stateName,
    optionalFields,
    replacesExisting:
      (!emptyInput && !same(node.input, input)) ||
      existing.some((binding) => !inputBindings.some((next) => same(binding, next))),
  };
}

export function useParentStateInput(document: Document, name: string): Document {
  assertDocument(document);
  const node = findNode(document.graph.root, name);
  if (!node) throw new Error('This activity no longer exists.');
  const { input, inputBindings } = parentStateInput(document, node);
  const next = replaceNode(document, name, { ...node, input, inputBindings });
  assertDocument(next);
  return next;
}
