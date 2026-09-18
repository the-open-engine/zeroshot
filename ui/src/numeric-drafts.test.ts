import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import { fileURLToPath } from 'node:url';
import { createNumericDrafts } from './numeric-drafts';
import { hasPendingEdits } from './pending-edits';
import { snapshotProfileSave } from './profile-save';
import type { Document } from './domain';

const document = (): Document => ({
  name: 'numeric-draft',
  graph: {
    profile: 'openengine.graph.full/v1',
    initialInput: { kind: 'null' },
    policy: {},
    root: { kind: 'step', name: 'worker' },
  },
  runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
});

test('an unfinished exponent survives inspector navigation and prevents saving a numeric prefix', (t) => {
  const drafts = createNumericDrafts();
  t.after(() => drafts.clear());
  const doc = document();
  const base = { node: 'worker', field: 'timeoutMs' as const, base: undefined };
  drafts.set({ ...base, text: '1' });
  assert.equal(doc.graph.root.timeoutMs, undefined, 'Typing a valid prefix does not commit it');
  drafts.set({ ...base, text: '1e' });
  // Neither changing selection nor hiding the inspector owns or clears these document drafts.
  assert.equal(drafts.get('other-node', 'timeoutMs'), undefined);
  assert.equal(drafts.get('worker', 'timeoutMs')?.text, '1e');
  let writes = 0;
  assert.equal(
    drafts.commit('worker', 'timeoutMs', true, () => {
      writes++;
    }),
    false
  );
  assert.equal(writes, 0);
  assert.throws(
    () => snapshotProfileSave(doc, { name: doc.name, generation: 1, pending: hasPendingEdits() }),
    /pending field edits/
  );
  assert.equal(doc.graph.root.timeoutMs, undefined);

  drafts.set({ ...base, text: '1e3' });
  drafts.commit('worker', 'timeoutMs', true, (value) => {
    doc.graph.root.timeoutMs = value;
  });
  assert.equal(doc.graph.root.timeoutMs, 1000);
  assert.equal(hasPendingEdits(), false);
  assert.equal(
    snapshotProfileSave(doc, { name: doc.name, generation: 1, pending: hasPendingEdits() }).request
      .graph.root.timeoutMs,
    1000
  );
});

test('explicit cancellation, node removal, JSON replacement and profile discard clean only their drafts', (t) => {
  const drafts = createNumericDrafts();
  t.after(() => drafts.clear());
  const add = (node: string) =>
    drafts.set({ node, field: 'timeoutMs', text: '1e', base: undefined });
  add('worker');
  add('other');
  drafts.clear('worker', 'timeoutMs');
  assert.equal(hasPendingEdits(), true);
  assert.equal(drafts.get('worker', 'timeoutMs'), undefined);
  drafts.prune(document().graph.root);
  assert.equal(hasPendingEdits(), false, 'Removing the remaining owning node removes its draft');
  add('worker');
  const changed = document().graph.root;
  changed.timeoutMs = 60000;
  drafts.prune(changed);
  assert.equal(hasPendingEdits(), false, 'An explicit graph value replaces the unfinished field');
  add('worker');
  drafts.clear();
  assert.equal(hasPendingEdits(), false, 'Discarding the document removes every pending number');
});

test('renaming a node moves its unfinished number without committing or losing it', (t) => {
  const drafts = createNumericDrafts();
  t.after(() => drafts.clear());
  drafts.set({ node: 'worker', field: 'timeoutMs', text: '1e', base: undefined });
  drafts.rename('worker', 'implementation');
  drafts.prune({ ...document().graph.root, name: 'implementation' });
  assert.equal(drafts.get('worker', 'timeoutMs'), undefined);
  assert.equal(drafts.get('implementation', 'timeoutMs')?.text, '1e');
  assert.equal(hasPendingEdits(), true);
});

test('reopening a numeric field renders the exact unfinished text after its component was replaced', async (t) => {
  const drafts = createNumericDrafts();
  t.after(() => drafts.clear());
  drafts.set({ node: 'worker', field: 'timeoutMs', text: '1e', base: undefined });
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false, ws: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  t.after(() => server.close());
  const { NumberField } = await server.ssrLoadModule('/src/NumberField.tsx');
  const { NumericDraftProvider } = await server.ssrLoadModule('/src/numeric-drafts.ts');
  const render = (node: string) =>
    renderToStaticMarkup(
      createElement(
        NumericDraftProvider,
        { value: drafts },
        createElement(NumberField, {
          node,
          field: 'timeoutMs',
          label: 'Timeout (ms)',
          value: undefined,
          optional: true,
          onChange: () => assert.fail('Rendering a field must not commit its draft'),
        })
      )
    );
  assert.doesNotMatch(render('other'), /value="1e"/);
  assert.match(render('worker'), /value="1e"/);
  assert.match(render('worker'), /aria-invalid="true"/);
  assert.equal(hasPendingEdits(), true);
});
