import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'vite';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';
import { createElement } from 'react';
import { newDocument } from './domain';

test('template runtime metadata keeps delivery bindings when creating a profile', () => {
  const template = {
    name: 'delivery',
    graph: {
      profile: 'openengine.graph.full/v1',
      initialInput: { kind: 'null' },
      policy: {},
      root: { kind: 'verifier', name: 'deliver' },
    },
    runtimeBindings: { deliver: { kind: 'git_delivery', connections: { github: ['GH_TOKEN'] } } },
  };
  const doc = newDocument(template);
  assert.deepEqual(doc.runtime.nodes.deliver, template.runtimeBindings.deliver);
  doc.runtime.nodes.deliver.connections!.github.push('EXTRA');
  assert.deepEqual(template.runtimeBindings.deliver.connections.github, ['GH_TOKEN']);
});

test('graph runtime configuration renders independently of the node inspector', async () => {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  try {
    const { RuntimeEditor } = await server.ssrLoadModule('/src/RuntimeEditor.tsx');
    const doc = {
      name: 'test',
      graph: {
        profile: 'full',
        initialInput: { kind: 'null' },
        policy: {},
        root: {
          kind: 'seq',
          name: 'run',
          children: [
            { kind: 'step', name: 'worker' },
            { kind: 'verifier', name: 'deliver' },
          ],
        },
      },
      runtime: {
        harness: 'codex',
        provider: 'openai',
        size: 'small',
        nodes: {
          worker: { kind: 'agent', model: 'custom-model' },
          deliver: { kind: 'git_delivery', connections: { github: ['GH_TOKEN'] } },
        },
      },
    };
    const html = renderToStaticMarkup(
      createElement(RuntimeEditor, {
        document: doc,
        schema: {},
        edit: () => {},
      })
    );
    assert.match(html, /Applies to the whole graph/);
    assert.match(html, /<label for="[^"]+" class="visually-hidden">Model for worker<\/label>/);
    assert.match(html, /value="custom-model"/);
    assert.doesNotMatch(
      html,
      /Git delivery/,
      'delivery needs no model, so it stays out of the way'
    );
    assert.doesNotMatch(html, /Model for deliver/);
    assert.doesNotMatch(html, /role="tab"/);
    assert.doesNotMatch(html, /aria-invalid/);

    const flagged = renderToStaticMarkup(
      createElement(RuntimeEditor, {
        document: doc,
        schema: {},
        invalidField: 'runtime.nodes.worker.model',
        edit: () => {},
      })
    );
    assert.equal(flagged.match(/aria-invalid="true"/g)?.length, 1);
    assert.match(flagged, /<input[^>]*aria-invalid="true"[^>]*value="custom-model"/);

    for (const [invalidField, validated] of [
      ['runtime.nodes.worker.model', { ...doc.runtime, nodes: { worker: { kind: 'agent' } } }],
      ['runtime.harness', { ...doc.runtime, harness: '' }],
    ] as const) {
      const draft = renderToStaticMarkup(
        createElement(RuntimeEditor, {
          document: doc,
          schema: {},
          invalidField,
          validated,
          edit: () => {},
        })
      );
      assert.doesNotMatch(
        draft,
        /aria-invalid/,
        `a draft edit clears the stale ${invalidField} flag`
      );
    }
    const unchanged = renderToStaticMarkup(
      createElement(RuntimeEditor, {
        document: doc,
        schema: {},
        invalidField: 'runtime.harness',
        validated: doc.runtime,
        edit: () => {},
      })
    );
    assert.equal(unchanged.match(/aria-invalid="true"/g)?.length, 1);
  } finally {
    await server.close();
  }
});

test('harness labels and provider choices follow the native schema, including future harnesses', async () => {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  try {
    const { RuntimeEditor } = await server.ssrLoadModule('/src/RuntimeEditor.tsx');
    const schema = {
      oneOf: [
        {
          properties: { harness: { const: 'codex' }, provider: { $ref: '#/$defs/CodexProvider' } },
        },
        {
          properties: {
            harness: { const: 'claude' },
            provider: { enum: ['anthropic', 'gateway'] },
          },
        },
        { properties: { harness: { const: 'copilot' }, provider: { enum: ['github'] } } },
        {
          properties: { harness: { enum: ['future-cli'] }, provider: { enum: ['future-service'] } },
        },
      ],
      $defs: { CodexProvider: { enum: ['openai', 'gateway'] }, RunSize: { enum: ['small'] } },
    };
    for (const [harness, providers] of [
      ['codex', ['openai', 'gateway']],
      ['claude', ['anthropic', 'gateway']],
      ['copilot', ['github']],
      ['future-cli', ['future-service']],
    ] as const) {
      const html = renderToStaticMarkup(
        createElement(RuntimeEditor, {
          document: {
            name: 'schema-choices',
            graph: {
              profile: 'openengine.graph.full/v1',
              initialInput: { kind: 'null' },
              policy: {},
              root: { kind: 'seq', name: 'run', children: [] },
            },
            runtime: { harness, provider: providers[0], size: 'small', nodes: {} },
          },
          schema,
          edit: () => {},
        })
      );
      const selects = [...html.matchAll(/<select[^>]*>(.*?)<\/select>/g)].map((match) => match[1]);
      const choices = (select: string) =>
        [...select.matchAll(/<option value="([^"]+)"/g)].map((match) => match[1]);
      assert.deepEqual(choices(selects[0]), ['codex', 'claude', 'copilot', 'future-cli']);
      assert.match(selects[0], /value="codex"[^>]*>Codex<\/option>/);
      assert.match(selects[0], /value="claude"[^>]*>Claude Code<\/option>/);
      assert.match(selects[0], /value="copilot"[^>]*>GitHub Copilot<\/option>/);
      assert.match(selects[0], /value="future-cli"[^>]*>future-cli<\/option>/);
      assert.deepEqual(choices(selects[1]), providers);
    }
  } finally {
    await server.close();
  }
});
