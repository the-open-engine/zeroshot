import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement, isValidElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import { fileURLToPath } from 'node:url';
import type { Document, GraphNode } from './domain';
import { freshGuard, newGuardKind } from './guards';
import { loopStopCandidates } from './loop-stop';

const verifier = (name: string): GraphNode => ({
  kind: 'verifier',
  name,
  signals: { verdict: ['accepted', 'rejected'] },
});
const worker = { kind: 'step', name: 'compose_report' };
const conditional = verifier('feasibility_review');
const loop: GraphNode = {
  kind: 'loop',
  name: 'rounds',
  maxIterations: 3,
  body: {
    kind: 'seq',
    name: 'round_body',
    children: [
      worker,
      {
        kind: 'choice',
        name: 'planning_execution',
        branches: [
          {
            when: {
              kind: 'in',
              value: { name: worker.name, source: 'error', field: null },
              labels: ['crash'],
            },
            node: { kind: 'fail', name: 'failed', reason: 'failed' },
          },
        ],
        otherwise: conditional,
      },
    ],
  },
};
const document = {
  graph: { root: { kind: 'seq', name: 'run', children: [verifier('earlier_review'), loop] } },
} as unknown as Document;

test('loop candidates exclude Agents/groups/outside results without excluding valid completing-iteration choices', () => {
  const before = structuredClone(document);
  assert.deepEqual(
    loopStopCandidates(loop).map((node) => node.name),
    ['feasibility_review']
  );
  const candidates = loopStopCandidates(loop);
  assert.equal(freshGuard(document, loop, candidates).value.name, conditional.name);
  assert.equal(
    newGuardKind('any', document, loop, candidates).guards[0].value.name,
    conditional.name
  );
  assert.equal(
    freshGuard(document, loop).value.name,
    'earlier_review',
    'generic decision defaults are unchanged'
  );
  assert.deepEqual(document, before);
});

test('availability for conditional, map and partial-join Verifiers stays with native admission', () => {
  const body: GraphNode = {
    kind: 'seq',
    name: 'body',
    children: [
      { kind: 'map', name: 'map', body: verifier('mapped') },
      { kind: 'par', name: 'first', join: { kind: 'any' }, branches: [verifier('partial')] },
      { kind: 'loop', name: 'nested', body: verifier('nested_review') },
    ],
  };
  assert.deepEqual(
    loopStopCandidates({ ...loop, body }).map((node) => node.name),
    ['mapped', 'partial', 'nested_review']
  );
  assert.deepEqual(loopStopCandidates({ kind: 'choice', name: 'choice' }), []);
});

test('UntilEditor preserves invalid authored references for repair, seeds eligible results and leaves decisions unrestricted', async () => {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false, ws: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  try {
    const { UntilEditor, GuardEditor } = await server.ssrLoadModule('/src/GuardEditor.tsx');
    const invalid = {
      kind: 'in',
      value: { name: worker.name, source: 'error', field: null },
      labels: ['crash'],
    };
    const before = structuredClone(invalid);
    const html = renderToStaticMarkup(
      createElement(UntilEditor, {
        document,
        node: loop,
        value: invalid,
        onChange: () => assert.fail('rendering must preserve authored references'),
      })
    );
    assert.match(html, /compose_report · error/);
    assert.match(html, /feasibility_review · verdict/);
    assert.doesNotMatch(html, /Loop exits require/);
    assert.doesNotMatch(html, /earlier_review · verdict/);
    assert.deepEqual(invalid, before);
    const valid = {
      ...invalid,
      value: { name: conditional.name, source: 'signal', field: 'verdict' },
      labels: ['accepted'],
    };
    const preserved = renderToStaticMarkup(
      createElement(UntilEditor, { document, node: loop, value: valid, onChange: () => {} })
    );
    assert.doesNotMatch(preserved, /feasibility_review \(unavailable\)/);
    const generic = renderToStaticMarkup(
      createElement(GuardEditor, { document, node: loop, value: invalid, onChange: () => {} })
    );
    assert.match(generic, /selected="">compose_report · error</);
    assert.match(generic, /earlier_review · verdict/);

    let next: any;
    const view = UntilEditor({
      document,
      node: loop,
      onChange: (value: any) => {
        next = value;
      },
    });
    let toggle: ReactElement<any> | undefined;
    function find(value: ReactNode) {
      if (Array.isArray(value)) return value.forEach(find);
      if (!isValidElement(value)) return;
      const element = value as ReactElement<any>;
      if (element.type === 'input' && element.props.type === 'checkbox') toggle = element;
      find(element.props.children);
    }
    find(view);
    assert.ok(toggle && !toggle.props.disabled);
    toggle.props.onChange({ target: { checked: true } });
    assert.equal(next.value.name, conditional.name);
  } finally {
    await server.close();
  }
});
