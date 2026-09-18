import test from 'node:test';
import assert from 'node:assert/strict';
import {
  addField,
  editableFieldType,
  fieldUpdate,
  flatType,
  removeField,
  renameField,
  typeSummary,
} from './schema';
import { suggestedModels } from './models';
const fixture = () => ({
  kind: 'record',
  metadata: { future: true },
  fields: {
    task: { type: { kind: 'string' }, required: true },
    nested: {
      type: { kind: 'record', fields: { x: { type: { kind: 'string' }, required: true } } },
      required: false,
      metadata: { untouched: true },
    },
  },
});
test('flat schema edits preserve nested schemas and additional metadata exactly', () => {
  const original = fixture();
  const changed = fieldUpdate(original, 'task', { type: { kind: 'number' } });
  assert.deepEqual(changed.fields.nested, original.fields.nested);
  assert.deepEqual(changed.metadata, original.metadata);
  assert.equal(original.fields.task.type.kind, 'string');
  assert.equal(flatType(original.fields.nested.type), false);
  assert.equal(flatType({ kind: 'array', items: original.fields.nested.type }), false);
  assert.equal(flatType({ kind: 'array', items: { kind: 'number' } }), true);
});
test('schema field names are literal and prototype-safe', () => {
  for (const name of ['a.b', '__proto__', 'constructor', 'toString']) {
    const changed = renameField(fixture(), 'task', name);
    assert.equal(Object.hasOwn(changed.fields, name), true);
    assert.equal(JSON.parse(JSON.stringify(changed)).fields[name].type.kind, 'string');
  }
  assert.throws(() => renameField(fixture(), 'task', 'nested'), /already exists/);
  for (const name of ['', '1field', 'a'.repeat(129)])
    assert.throws(() => renameField(fixture(), 'task', name));
  assert.equal(
    Object.hasOwn(renameField(fixture(), 'task', 'a'.repeat(128)).fields, 'a'.repeat(128)),
    true
  );
});
test('adding fields avoids collisions; removing last field retains empty object schema', () => {
  const one = addField({ kind: 'record', fields: {} });
  assert.equal(Object.hasOwn(addField(one).fields, 'field_2'), true);
  assert.deepEqual(removeField(one, 'field'), { kind: 'record', fields: {} });
});
test('visual array item records expose one field level and preserve deeper imported types', () => {
  const items = fixture();
  const schema = { kind: 'array', items };
  assert.equal(flatType(schema), false);
  assert.equal(editableFieldType(schema), true);
  assert.equal(editableFieldType(schema, false), false);
  assert.equal(editableFieldType(items), false, 'ordinary nested object fields stay advanced');
  assert.equal(editableFieldType({ ...schema, future: true }), false);
  for (const invalid of [null, [], {}, { kind: 'record' }, { kind: 'record', fields: [] }])
    assert.equal(editableFieldType({ kind: 'array', items: invalid }), false);
  const changed = {
    ...schema,
    items: renameField(fieldUpdate(items, 'task', { required: false }), 'task', 'name'),
  };
  assert.deepEqual(changed.items.fields.nested, items.fields.nested);
  assert.deepEqual(changed.items.metadata, items.metadata);
  assert.equal(changed.items.fields.name.required, false);
  assert.equal(items.fields.task.required, true);
  assert.deepEqual(JSON.parse(JSON.stringify(changed)), changed);
});
test('node summaries handle unknown and malformed schemas without recursive rendering', () => {
  for (const value of [
    null,
    [],
    {},
    { kind: '__proto__' },
    { kind: 'array', items: { kind: '__proto__' } },
    { kind: 'array', items: { kind: ['string'] } },
  ])
    assert.equal(typeof typeSummary(value), 'string');
  assert.equal(
    typeSummary({ kind: 'record', fields: { x: { type: { kind: 'string' }, required: false } } }),
    'x?: string'
  );
});
test('model suggestions depend on both harness and provider, accepting no inferred fallback', () => {
  assert.ok(suggestedModels('codex', 'openai').includes('gpt-5.6-sol'));
  assert.ok(suggestedModels('claude', 'openrouter').includes('anthropic/claude-sonnet-5'));
  assert.deepEqual(suggestedModels('claude', 'openai'), []);
  assert.deepEqual(suggestedModels('__proto__', 'constructor'), []);
});
