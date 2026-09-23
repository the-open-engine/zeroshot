import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { findNode, type Document } from './domain';
import { createViteTestServer } from './test-support';

const record = (fields: Record<string, any>) => ({
  kind: 'record',
  fields: Object.fromEntries(
    Object.entries(fields).map(([name, type]) => [name, { type, required: true }])
  ),
});
function fixture(): Document {
  const state = record({ task: { kind: 'string' }, plan: { kind: 'string' } });
  return {
    name: 'inspector-test',
    graph: {
      profile: 'openengine.graph.full/v1',
      initialInput: record({ task: { kind: 'string' } }),
      policy: {},
      root: {
        kind: 'seq',
        name: 'run',
        state,
        promotedStatePaths: [],
        children: [
          {
            kind: 'step',
            name: 'planner',
            worker: 'agent.planner@1',
            instructions: 'Write a plan.',
            attempts: 1,
            input: record({ task: { kind: 'string' } }),
            output: record({ plan: { kind: 'string' } }),
            inputBindings: [{ target: ['task'], value: { source: 'state', path: ['task'] } }],
            writeBindings: [
              { target: ['plan'], value: { node: 'planner', channel: 'out', path: ['plan'] } },
            ],
          },
          {
            kind: 'verifier',
            name: 'reviewer',
            worker: 'agent.reviewer@1',
            instructions: 'Review the plan.',
            attempts: 1,
            input: record({ brief: { kind: 'string' } }),
            output: record({ score: { kind: 'number' } }),
            diagnostic: record({ feedback: { kind: 'string' } }),
            signals: { verdict: ['accepted', 'rejected'] },
            inputBindings: [{ target: ['brief'], value: { source: 'state', path: ['plan'] } }],
            writeBindings: [],
          },
        ],
      },
    },
    runtime: {
      harness: 'codex',
      provider: 'openai',
      size: 'small',
      nodes: {
        planner: { kind: 'agent', model: 'custom-model' },
        reviewer: { kind: 'agent', model: 'custom-model' },
      },
    },
  };
}
async function inspector(t: TestContext) {
  const server = await createViteTestServer(t);
  const { Inspector } = await server.ssrLoadModule('/src/Inspector.tsx');
  const { createNumericDrafts, NumericDraftProvider } =
    await server.ssrLoadModule('/src/numeric-drafts.ts');
  return (props: any) =>
    createElement(
      NumericDraftProvider,
      { value: createNumericDrafts() },
      createElement(Inspector, props)
    );
}
function props(document: Document, name: string) {
  return {
    document,
    node: findNode(document.graph.root, name),
    parent: name === 'run' ? undefined : document.graph.root,
    schema: {},
    workers: [],
    edit: () => assert.fail('Rendering must not edit the profile'),
    updateNode: () => assert.fail('Rendering must not edit the profile'),
    openJson: () => {},
    move: () => {},
    remove: () => {},
    close: () => {},
    replaceBody: () => {},
    wrapBody: () => {},
    selectNode: () => {},
    addOtherwise: () => {},
  };
}

test('Inspector presents connected Inputs and one merged Outputs section without mapping machinery', async (t) => {
  const Inspector = await inspector(t),
    document = fixture(),
    before = structuredClone(document);
  const html = renderToStaticMarkup(createElement(Inspector, props(document, 'reviewer')));
  assert.equal((html.match(/aria-label="Inputs"/g) ?? []).length, 1);
  assert.equal((html.match(/aria-label="Outputs"/g) ?? []).length, 1);
  assert.match(html, /aria-label="Source for brief"/);
  assert.match(html, /planner · plan/);
  assert.match(html, /aria-label="Output score name"/);
  assert.match(html, /aria-label="Output feedback name"/);
  assert.match(html, /aria-label="Outcome verdict name"/);
  assert.match(html, /accepted, rejected/);
  for (const removed of [
    'Scoped data',
    'Advanced mappings',
    'Data connections',
    'Use parent state',
    'Copy parent state',
    'State writes',
    'Verifier signals',
    '>Diagnostic<',
  ])
    assert.equal(
      html.includes(removed),
      false,
      `${removed} must not return to the normal inspector`
    );
  assert.deepEqual(document, before);
});

test('root input fields and array item schemas stay visually editable through the unified editor', async (t) => {
  const Inspector = await inspector(t),
    document = fixture();
  document.graph.initialInput.fields.items = {
    type: { kind: 'array', items: record({ title: { kind: 'string' } }) },
    required: true,
  };
  const html = renderToStaticMarkup(createElement(Inspector, props(document, 'run')));
  assert.match(html, /aria-label="Input task name"/);
  assert.match(html, /aria-label="Input items item type"/);
  assert.match(html, /aria-label="Input items item title name"/);
  assert.match(html, /Add input/);
  assert.equal(html.includes('Copy run inputs'), false);
  assert.equal(html.includes('Input mappings'), false);
});

test('unfamiliar output declarations render a raw control without discarding the saved values', async (t) => {
  const Inspector = await inspector(t),
    document = fixture(),
    reviewer = findNode(document.graph.root, 'reviewer')!;
  reviewer.output.fields.future = {
    type: { kind: 'future', details: ['preserve'] },
    required: false,
  };
  reviewer.diagnostic.fields.incomplete = null;
  const before = structuredClone(document);
  const html = renderToStaticMarkup(createElement(Inspector, props(document, 'reviewer')));
  assert.match(html, /aria-label="Output future name"/);
  assert.match(html, /aria-label="Outputs JSON"/);
  assert.equal(html.includes('unsupported format'), false);
  assert.deepEqual(document, before);
});

test('Inspector offers explicit writing/read-only conversion only for editable Agent contracts', async (t) => {
  const Inspector = await inspector(t),
    document = fixture();
  assert.match(
    renderToStaticMarkup(createElement(Inspector, props(document, 'planner'))),
    />Make read-only Verifier<\/button>/
  );
  assert.match(
    renderToStaticMarkup(createElement(Inspector, props(document, 'reviewer'))),
    />Make writing Agent<\/button>/
  );
  for (const kind of ['git_delivery', 'future_worker']) {
    document.runtime.nodes.reviewer = { kind };
    assert.doesNotMatch(
      renderToStaticMarkup(createElement(Inspector, props(document, 'reviewer'))),
      /Make writing Agent/
    );
  }
  document.runtime.nodes.reviewer = { kind: 'agent', model: 'opaque-model' };
  findNode(document.graph.root, 'reviewer')!.worker = 'builtin.git-delivery.merge@2';
  assert.doesNotMatch(
    renderToStaticMarkup(createElement(Inspector, props(document, 'reviewer'))),
    /Make writing Agent/
  );
});

test('only agent-backed Verifiers expose supported attempt choices; imported values stay untouched', async (t) => {
  const Inspector = await inspector(t),
    document = fixture();
  const planner = findNode(document.graph.root, 'planner')!;
  for (const attempts of [1, 2]) {
    planner.attempts = attempts;
    const before = structuredClone(document);
    const html = renderToStaticMarkup(createElement(Inspector, props(document, 'planner')));
    assert.doesNotMatch(html, />Attempts<\/label>/);
    assert.deepEqual(document, before);
  }
  const reviewer = findNode(document.graph.root, 'reviewer')!;
  for (const attempts of [1, 2]) {
    reviewer.attempts = attempts;
    const html = renderToStaticMarkup(createElement(Inspector, props(document, 'reviewer')));
    const control = html.match(/<label[^>]*>Attempts<\/label>(.*?)<\/div>/)?.[1];
    assert.ok(control);
    assert.deepEqual(
      [...control.matchAll(/<option value="([^"]+)"/g)].map((match) => match[1]),
      ['1', '2']
    );
    assert.doesNotMatch(control, /type="number"/);
  }
  reviewer.attempts = 3;
  const before = structuredClone(document);
  assert.match(
    renderToStaticMarkup(createElement(Inspector, props(document, 'reviewer'))),
    /aria-label="Attempts JSON"/
  );
  assert.deepEqual(document, before);
  for (const kind of ['git_delivery', 'future_worker']) {
    document.runtime.nodes.reviewer = { kind };
    assert.doesNotMatch(
      renderToStaticMarkup(createElement(Inspector, props(document, 'reviewer'))),
      />Attempts<\/label>/
    );
  }
});
