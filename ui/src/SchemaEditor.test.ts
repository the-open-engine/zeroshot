import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement, isValidElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import { fileURLToPath } from 'node:url';
import { bindingContext, dataChoices, type BindingChoice } from './bindings';
import type { Document } from './domain';

test('visual array item controls author record fields that native map selectors can address', async () => {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false, ws: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  try {
    const { SchemaEditor } = await server.ssrLoadModule('/src/SchemaEditor.tsx');
    let schema: any = {
      kind: 'record',
      fields: {
        suites: { type: { kind: 'array', items: { kind: 'string' } }, required: true },
      },
    };
    let edits = 0;
    const props = () => ({
      label: 'Run inputs',
      value: schema,
      onChange: (next: any) => {
        schema = next;
        edits++;
      },
      openJson: () => assert.fail('record items must be editable without JSON'),
    });
    function interact(
      match: (element: ReactElement<any>) => boolean,
      action: (props: any) => void
    ) {
      let control: ReactElement<any> | undefined;
      function find(value: ReactNode) {
        if (Array.isArray(value)) return value.forEach(find);
        if (!isValidElement<{ children?: ReactNode }>(value)) return;
        if (match(value)) control = value;
        if (typeof value.type === 'function') find((value.type as any)(value.props));
        else find(value.props.children);
      }
      function Exercise() {
        const view = SchemaEditor(props());
        find(view);
        return null;
      }
      renderToStaticMarkup(createElement(Exercise));
      assert.ok(control, 'requested control must be rendered');
      action(control.props);
    }
    const initial = structuredClone(schema);
    const markup = renderToStaticMarkup(createElement(SchemaEditor, props()));
    assert.match(markup, /value="record">Object/);
    assert.equal(edits, 0);
    assert.deepEqual(schema, initial);

    interact(
      (element) => element.props['aria-label'] === 'Run inputs field suites item type',
      (control) => control.onChange({ target: { value: 'record' } })
    );
    assert.deepEqual(schema.fields.suites.type.items, { kind: 'record', fields: {} });
    interact(
      (element) =>
        element.type === 'button' &&
        Array.isArray(element.props.children) &&
        element.props.children.includes('Add item field'),
      (control) => control.onClick()
    );
    assert.deepEqual(schema.fields.suites.type.items.fields.field, {
      type: { kind: 'string' },
      required: true,
    });
    interact(
      (element) => element.props['aria-label'] === 'Run inputs field suites item field field type',
      (control) => control.onChange({ target: { value: 'integer' } })
    );
    assert.equal(schema.fields.suites.type.items.fields.field.type.kind, 'integer');
    interact(
      (element) =>
        element.props['aria-label'] === 'Run inputs field suites item field field required',
      (control) => control.onChange({ target: { checked: false } })
    );
    assert.equal(schema.fields.suites.type.items.fields.field.required, false);
    assert.deepEqual(JSON.parse(JSON.stringify(schema)), schema);

    const child = { kind: 'step', name: 'inspect', input: { kind: 'null' } };
    const map = {
      kind: 'map',
      name: 'suites',
      state: schema,
      over: { source: 'state', path: ['suites'] },
      body: child,
    };
    const document = { graph: { initialInput: schema, root: map } } as unknown as Document;
    const choices: BindingChoice[] = dataChoices(bindingContext(document, child));
    assert.ok(
      choices.some(
        (choice) => choice.value.source === 'item' && choice.value.path.join('.') === 'field'
      )
    );
    assert.equal(edits, 4);
  } finally {
    await server.close();
  }
});

test('record item rendering preserves advanced child schemas and unknown metadata', async () => {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false, ws: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  try {
    const { SchemaEditor } = await server.ssrLoadModule('/src/SchemaEditor.tsx');
    const schema = {
      kind: 'array',
      items: {
        kind: 'record',
        metadata: { version: 2 },
        fields: {
          name: { type: { kind: 'string' }, required: true },
          details: {
            type: {
              kind: 'record',
              fields: { nested: { type: { kind: 'string' }, required: true } },
            },
            required: false,
            metadata: { opaque: true },
          },
          nestedItems: {
            type: { kind: 'array', items: { kind: 'record', fields: {} } },
            required: false,
          },
        },
      },
    };
    const before = structuredClone(schema);
    const markup = renderToStaticMarkup(
      createElement(SchemaEditor, {
        label: 'Input',
        value: schema,
        onChange: () => assert.fail('rendering must not convert any schema'),
        openJson: () => {},
      })
    );
    assert.match(markup, /aria-label="Input item field name name"/);
    assert.match(markup, /aria-label="Input item field details type"[^>]*disabled/);
    assert.match(markup, /aria-label="Input item field nestedItems type"[^>]*disabled/);
    assert.doesNotMatch(markup, /aria-label="Input item field details field nested name"/);
    assert.deepEqual(schema, before);
  } finally {
    await server.close();
  }
});
