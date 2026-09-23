import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { type Document, type GraphNode } from './domain';
import { createViteTestServer } from './test-support';

async function editor(t: TestContext) {
  const server = await createViteTestServer(t);
  return server.ssrLoadModule('/src/NodeDataEditor.tsx');
}

test('rapid data edits reject visibly while the first request owns the lock, then allow retry', async (t) => {
  const { withDataEditLock } = await editor(t);
  const pending = { current: false };
  const writes: string[] = [];
  let finish!: () => void;
  const response = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const first = withDataEditLock(pending, async () => {
    await response;
    writes.push('kitPath');
  });
  const second = () =>
    withDataEditLock(pending, async () => {
      writes.push('checklistPath');
    });
  assert.equal(pending.current, true);
  await assert.rejects(second(), /still applying.*retry this edit/);
  assert.equal(pending.current, true, 'A rejected overlap must not unlock the first request');
  assert.deepEqual(writes, []);
  finish();
  await first;
  assert.equal(pending.current, false);
  await second();
  assert.deepEqual(writes, ['kitPath', 'checklistPath']);
});

test('failed data requests release the lock without swallowing their error', async (t) => {
  const { withDataEditLock } = await editor(t);
  const pending = { current: false };
  await assert.rejects(
    withDataEditLock(pending, async () => {
      throw new Error('Native source is unavailable');
    }),
    /Native source is unavailable/
  );
  assert.equal(pending.current, false);
  let retried = false;
  await withDataEditLock(pending, async () => {
    retried = true;
  });
  assert.equal(retried, true);
});

test('input and run-output source pickers explicitly disable every edit control while applying', async (t) => {
  const { BoundFields } = await editor(t);
  const schema = {
    kind: 'record',
    fields: {
      kitPath: { required: true, type: { kind: 'string' } },
      checklistPath: { required: true, type: { kind: 'string' } },
    },
  };
  for (const title of ['Inputs', 'Outputs']) {
    const node: GraphNode = {
      kind: title === 'Inputs' ? 'verifier' : 'succeed',
      name: 'review',
      input: schema,
      output: schema,
      inputBindings: [],
      bindings: [],
    };
    const document: Document = {
      name: 'launch',
      graph: {
        profile: 'test',
        policy: {},
        initialInput: schema,
        root: { kind: 'seq', name: 'run', state: schema, children: [node], promotedStatePaths: [] },
      },
      runtime: { harness: '', provider: '', size: 'small', nodes: {} },
    };
    const props = {
      title,
      document,
      node,
      change: async () => {},
      rename: () => {},
      raw: () => {},
    };
    const pending = renderToStaticMarkup(createElement(BoundFields, { ...props, busy: true }));
    const controls = pending.match(/<(?:select|input|button)\b[^>]*>/g)!;
    const sourceSelects = controls.filter((control) => control.startsWith('<select'));
    assert.equal(sourceSelects.length, 3);
    assert.ok(sourceSelects.every((control) => control.includes('disabled=""')));
    assert.ok(
      controls
        .filter((control) => /aria-label="(?:Remove|Input|Output) /.test(control))
        .every((control) => control.includes('disabled=""'))
    );
    const ready = renderToStaticMarkup(createElement(BoundFields, { ...props, busy: false }));
    assert.ok(
      (ready.match(/<select\b[^>]*>/g) ?? []).every((control) => !control.includes('disabled'))
    );
  }
});
