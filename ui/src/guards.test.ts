import test from 'node:test';
import assert from 'node:assert/strict';
import {
  addSignal,
  appendSignalLabel,
  changeSignalLabel,
  controlFields,
  controlLabels,
  controlSources,
  editableGuard,
  editableJoin,
  freshGuard,
  newGuardKind,
  positiveCount,
  renameSignal,
  selectControl,
  wrapGuard,
} from './guards';
import { type Document, type GraphNode } from './domain';

const worker: GraphNode = { kind: 'step', name: 'worker' };
const acceptance: GraphNode = {
  kind: 'verifier',
  name: 'acceptance',
  signals: { verdict: ['accepted', 'rejected'] },
};
const code: GraphNode = { ...acceptance, name: 'code' };
const document: Document = {
  name: 'conditions',
  graph: {
    profile: 'openengine.graph.full/v1',
    initialInput: { kind: 'null' },
    policy: {},
    root: { kind: 'seq', name: 'run', children: [worker, acceptance, code] },
  },
  runtime: { harness: '', provider: '', size: 'small', nodes: {} },
};

test('standard software-change review guards can be built from visual selector choices', () => {
  const workerFailure = {
    ...freshGuard(document),
    value: selectControl(worker),
    labels: controlLabels(document, selectControl(worker)),
  };
  assert.deepEqual(workerFailure, {
    kind: 'in',
    value: { name: 'worker', source: 'error', field: null },
    labels: ['crash', 'malformed', 'refusal', 'timeout'],
  });
  const reviewerFailure = {
    ...newGuardKind('any', document),
    guards: [acceptance, code].map((node) => ({
      kind: 'in',
      value: selectControl(node, 'error'),
      labels: workerFailure.labels,
    })),
  };
  assert.equal(reviewerFailure.kind, 'any');
  assert.deepEqual(
    reviewerFailure.guards.map((guard) => guard.value.name),
    ['acceptance', 'code']
  );
  const accepted = {
    ...newGuardKind('all', document),
    guards: [acceptance, code].map((node) => ({
      kind: 'in',
      value: selectControl(node, 'signal'),
      labels: ['accepted'],
    })),
  };
  assert.deepEqual(accepted, {
    kind: 'all',
    guards: [
      {
        kind: 'in',
        value: { name: 'acceptance', source: 'signal', field: 'verdict' },
        labels: ['accepted'],
      },
      {
        kind: 'in',
        value: { name: 'code', source: 'signal', field: 'verdict' },
        labels: ['accepted'],
      },
    ],
  });
  for (const guard of accepted.guards)
    assert.ok(controlLabels(document, guard.value).includes('accepted'));
});

test('control choices follow native worker, verifier and group result domains', () => {
  assert.deepEqual(controlSources(worker), ['error']);
  assert.deepEqual(controlSources(acceptance), ['error', 'signal']);
  assert.deepEqual(controlSources(document.graph.root), []);
  assert.deepEqual(controlFields({ kind: 'loop', name: 'loop' }), {
    terminated: ['converged', 'exhausted'],
  });
  assert.deepEqual(controlFields({ kind: 'map', name: 'map' }), { overflow: ['ok', 'overflow'] });
  assert.deepEqual(
    controlFields({ kind: 'par', name: 'reviews', join: { kind: 'quorum', count: 1 } }),
    { joined: ['reached', 'quorum_unreachable'] }
  );
  assert.deepEqual(
    controlFields({
      kind: 'par',
      name: 'reviews',
      join: { kind: 'first', when: freshGuard(document) },
    }),
    { raced: ['satisfied', 'no_satisfier'] }
  );
  assert.deepEqual(controlLabels(document, { name: 'missing', source: 'error' }), []);
  assert.deepEqual(
    controlLabels(document, { name: 'worker', source: 'error', field: 'unsupported' }),
    []
  );
  assert.deepEqual(
    controlLabels(document, { name: 'worker', source: 'signal', field: 'verdict' }),
    []
  );
  assert.deepEqual(
    controlLabels(document, { name: 'acceptance', source: 'signal', field: 'missing' }),
    []
  );
});

test('threshold guards use canonical k_of_n and k_of_map wire shapes and positive counts', () => {
  const leaf = freshGuard(document);
  assert.deepEqual(newGuardKind('k_of_n', document), {
    kind: 'k_of_n',
    count: 1,
    values: [leaf.value],
    labels: leaf.labels,
  });
  assert.deepEqual(newGuardKind('k_of_map', document), {
    kind: 'k_of_map',
    count: 1,
    value: leaf.value,
    labels: leaf.labels,
  });
  assert.equal(positiveCount('3'), 3);
  for (const invalid of ['', '0', '-1', '1.5', 'NaN', '9007199254740992'])
    assert.throws(() => positiveCount(invalid));
});

test('wrapping and switching boolean groups preserve unknown nested wire data', () => {
  const future = { kind: 'future_guard', condition: { domain: ['future'] } };
  assert.equal(editableGuard(future), false);
  const wrapped = wrapGuard(future, 'all')!;
  assert.deepEqual(wrapped, { kind: 'all', guards: [future] });
  const switched = wrapGuard({ ...wrapped, extension: { keep: true } }, 'any');
  assert.deepEqual(switched, { kind: 'any', guards: [future], extension: { keep: true } });
  assert.deepEqual(wrapGuard(future, 'not'), { kind: 'not', guard: future });
  assert.equal(wrapGuard(future, 'in'), undefined);
  assert.deepEqual(future, { kind: 'future_guard', condition: { domain: ['future'] } });
  assert.equal(editableGuard({ kind: 'in', value: null, labels: ['accepted'] }), false);
  assert.equal(editableGuard({ kind: 'all', guards: 'unsupported' }), false);
});

test('signals build the standard verdict and keep unrelated declarations through edits', () => {
  const future = { futureSignal: { version: 2 } };
  const signals = addSignal(future);
  assert.deepEqual(signals, { ...future, verdict: ['accepted', 'rejected'] });
  const renamed = renameSignal(signals, 'verdict', 'review');
  assert.deepEqual(changeSignalLabel(renamed, 'review', 1, 'needs_work'), {
    ...future,
    review: ['accepted', 'needs_work'],
  });
  assert.deepEqual(appendSignalLabel({ verdict: ['label', 'label_2'] }, 'verdict'), {
    verdict: ['label', 'label_2', 'label_3'],
  });
  assert.deepEqual(signals.verdict, ['accepted', 'rejected']);
  assert.throws(() => renameSignal(signals, 'verdict', 'futureSignal'), /already exists/);
  assert.throws(() => renameSignal(signals, 'verdict', 'bad name'));
  assert.throws(() => changeSignalLabel(signals, 'verdict', 1, 'accepted'), /already exists/);
  assert.throws(() => changeSignalLabel(signals, 'verdict', 0, ''));
  assert.throws(() => appendSignalLabel(signals, 'futureSignal'), /not editable/);
});

test('the guard result domain offers verifier-authored signal names without a fixed catalog', () => {
  const custom: GraphNode = {
    kind: 'verifier',
    name: 'custom',
    signals: { security: ['safe', 'review_needed'] },
  };
  const customDocument = {
    ...document,
    graph: { ...document.graph, root: { ...document.graph.root, children: [custom] } },
  };
  const selector = selectControl(custom, 'signal');
  assert.deepEqual(selector, { name: 'custom', source: 'signal', field: 'security' });
  assert.deepEqual(controlLabels(customDocument, selector), ['safe', 'review_needed']);
});

test('malformed selector fields, labels, counts and discriminators stay out of visual controls', () => {
  const leaf = freshGuard(document);
  for (const field of ['name', 'source', 'field']) {
    for (const invalid of [{ bad: true }, ['unexpected'], 7, false]) {
      const malformed = { ...leaf, value: { ...leaf.value, [field]: invalid } };
      assert.equal(editableGuard(malformed), false, `malformed selector ${field}`);
      assert.equal(
        editableGuard({ ...newGuardKind('k_of_n', document), values: [malformed.value] }),
        false
      );
    }
  }
  for (const invalid of [null, {}, ['crash', { bad: true }], [null], [7]]) {
    assert.equal(editableGuard({ ...leaf, labels: invalid }), false);
  }
  for (const invalid of [
    undefined,
    null,
    {},
    { toString: 'not callable' },
    '1',
    0,
    -1,
    1.5,
    Infinity,
    Number.MAX_SAFE_INTEGER + 1,
  ]) {
    assert.equal(editableGuard({ ...newGuardKind('k_of_n', document), count: invalid }), false);
    assert.equal(editableGuard({ ...newGuardKind('k_of_map', document), count: invalid }), false);
    assert.equal(editableJoin({ kind: 'quorum', count: invalid }), false);
  }
  for (const kind of [null, [], {}, { toString: 'not callable' }, { toString: null }]) {
    assert.equal(editableGuard({ ...leaf, kind }), false);
    assert.equal(editableJoin({ kind }), false);
  }
  assert.equal(editableGuard({ ...leaf, value: { ...leaf.value, field: null } }), true);
  assert.equal(editableGuard({ ...leaf, value: { name: 'worker', source: 'error' } }), true);
});

test('malformed imported guards, joins and signal declarations render preserved fallbacks', async (t) => {
  const { createServer } = await import('vite');
  const { fileURLToPath } = await import('node:url');
  const { createElement } = await import('react');
  const { renderToStaticMarkup } = await import('react-dom/server');
  // Use the application's transform pipeline so this regression renders the real TSX and CSS imports.
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false, ws: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  t.after(() => server.close());
  const { GuardEditor, JoinEditor, UntilEditor } =
    await server.ssrLoadModule('/src/GuardEditor.tsx');
  const { SignalsEditor } = await server.ssrLoadModule('/src/SignalsEditor.tsx');
  const onChange = () => assert.fail('Rendering must never rewrite imported values.');
  const malformed = {
    kind: 'in',
    value: { name: 'worker', source: { bad: true }, field: null },
    labels: ['crash'],
  };
  const original = structuredClone(malformed);
  const guardMarkup = renderToStaticMarkup(
    createElement(GuardEditor, { document, value: malformed, onChange })
  );
  assert.match(guardMarkup, /Custom condition/);
  assert.doesNotMatch(guardMarkup, /result 1 source/);
  assert.deepEqual(malformed, original);
  for (const value of [
    { kind: { toString: 'not callable' } },
    { kind: { toString: null } },
    { kind: 'k_of_n', values: [selectControl(worker)], labels: ['crash'], count: { bad: true } },
    { kind: 'all', guards: [malformed, { kind: { toString: 'not callable' } }, null] },
  ]) {
    assert.match(
      renderToStaticMarkup(createElement(GuardEditor, { document, value, onChange })),
      /Custom condition/
    );
    assert.match(
      renderToStaticMarkup(createElement(UntilEditor, { document, value, onChange })),
      /Custom condition/
    );
  }
  for (const value of [
    { kind: { toString: 'not callable' } },
    { kind: { toString: null } },
    { kind: 'quorum', count: { toString: 'not callable' } },
  ]) {
    assert.match(
      renderToStaticMarkup(createElement(JoinEditor, { document, value, onChange })),
      /join is preserved unchanged/
    );
  }
  for (const value of [
    null,
    ['unexpected'],
    { verdict: ['accepted', { bad: true }] },
    { verdict: { bad: true } },
    { toString: null },
  ]) {
    assert.match(
      renderToStaticMarkup(createElement(SignalsEditor, { value, onChange })),
      /preserved unchanged/
    );
  }
});
