// Presentation helpers for the Rust PayloadType wire format. Admission stays in Rust.
export type Payload = { kind: string; [key: string]: any };
export const scalarKinds = ['null', 'string', 'boolean', 'integer', 'number'];
export const typeLabels: Record<string, string> = {
  null: 'None',
  string: 'String',
  boolean: 'Boolean',
  integer: 'Integer',
  number: 'Number',
  record: 'Object',
  enum: 'Enum',
  array: 'Array',
};
const object = (v: any) => !!v && typeof v === 'object' && !Array.isArray(v);
export function flatType(value: any): boolean {
  if (!object(value)) return false;
  if (scalarKinds.includes(value.kind)) return Object.keys(value).every((k) => k === 'kind');
  if (value.kind === 'enum')
    return (
      Object.keys(value).every((k) => ['kind', 'values'].includes(k)) &&
      Array.isArray(value.values) &&
      value.values.every((v: unknown) => typeof v === 'string')
    );
  if (value.kind === 'array')
    return (
      Object.keys(value).every((k) => ['kind', 'items'].includes(k)) &&
      object(value.items) &&
      scalarKinds.includes(value.items.kind) &&
      flatType(value.items)
    );
  return false;
}
// Record items expose field paths that native map bindings can select. Limit
// visual nesting to one item object; deeper imported types stay lossless.
export function editableFieldType(value: any, allowObjectItems = true): boolean {
  return (
    flatType(value) ||
    (allowObjectItems &&
      object(value) &&
      value.kind === 'array' &&
      Object.keys(value).every((key) => ['kind', 'items'].includes(key)) &&
      !!recordFields(value.items))
  );
}
export function recordFields(value: any): Record<string, any> | undefined {
  return object(value) && value.kind === 'record' && object(value.fields)
    ? value.fields
    : undefined;
}
export function freshType(kind: string): Payload {
  if (kind === 'record') return { kind, fields: {} };
  if (kind === 'enum') return { kind, values: ['value'] };
  if (kind === 'array') return { kind, items: { kind: 'string' } };
  return { kind };
}
export function renameField(schema: Payload, before: string, after: string): Payload {
  if (!/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(after) || after.length > 128)
    throw new Error(
      'Use a letter or underscore, then up to 127 letters, numbers, dots, dashes or underscores.'
    );
  const fields = recordFields(schema);
  if (!fields || !Object.hasOwn(fields, before)) throw new Error('This field no longer exists.');
  if (after !== before && Object.hasOwn(fields, after))
    throw new Error('A field with this name already exists.');
  return {
    ...schema,
    fields: Object.fromEntries(
      Object.entries(fields).map(([name, field]) => [name === before ? after : name, field])
    ),
  };
}
export function addField(schema: Payload): Payload {
  const fields = recordFields(schema);
  if (!fields) throw new Error('Select Object to add fields.');
  let name = 'field',
    suffix = 2;
  while (Object.hasOwn(fields, name)) name = `field_${suffix++}`;
  return { ...schema, fields: { ...fields, [name]: { type: { kind: 'string' }, required: true } } };
}
export function fieldUpdate(schema: Payload, name: string, patch: Record<string, any>): Payload {
  const fields = recordFields(schema);
  if (!fields || !Object.hasOwn(fields, name)) throw new Error('This field no longer exists.');
  return { ...schema, fields: { ...fields, [name]: { ...fields[name], ...patch } } };
}
export function removeField(schema: Payload, name: string): Payload {
  return {
    ...schema,
    fields: Object.fromEntries(
      Object.entries(recordFields(schema) ?? {}).filter(([key]) => key !== name)
    ),
  };
}
export function typeSummary(value: any): string {
  if (!object(value) || typeof value.kind !== 'string') return 'Unspecified';
  if (value.kind === 'record') {
    const fields = recordFields(value);
    if (!fields) return 'Object';
    const entries = Object.entries(fields);
    if (!entries.length) return 'Empty object';
    return (
      entries
        .slice(0, 4)
        .map(([name, f]) => `${name}${f?.required === false ? '?' : ''}: ${shortType(f?.type)}`)
        .join(', ') + (entries.length > 4 ? `, +${entries.length - 4}` : '')
    );
  }
  return shortType(value);
}
function shortType(value: any): string {
  if (!object(value) || typeof value.kind !== 'string') return '?';
  if (value.kind === 'array') return `${safeLabel(value.items?.kind)}[]`;
  return safeLabel(value.kind);
}
function safeLabel(kind: unknown): string {
  return typeof kind === 'string' && Object.hasOwn(typeLabels, kind)
    ? typeLabels[kind].toLowerCase()
    : 'advanced';
}
