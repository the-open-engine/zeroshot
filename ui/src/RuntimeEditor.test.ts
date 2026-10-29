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

test('graph runtime configuration renders independently of the node inspector', () =>
  withRuntimeEditor((RuntimeEditor) => {
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
    assert.match(html, /Defaults for agents without a harness and provider override/);
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
  }));

test('harness labels and provider choices follow the host catalog, including contributions', () =>
  withRuntimeEditor((RuntimeEditor) => {
    const harnesses = [
      { id: 'codex', label: 'Codex', providers: ['openai', 'gateway'] },
      { id: 'claude', label: 'Claude Code', providers: ['anthropic', 'gateway'] },
      { id: 'copilot', label: 'GitHub Copilot', providers: ['github'] },
      {
        id: 'future-cli',
        label: 'Future CLI',
        providers: ['future-service'],
        source: 'contributed',
      },
    ];
    const schema = {
      oneOf: [{ properties: { harness: { const: 'schema-only-cli' } } }],
      $defs: { RunSize: { enum: ['small'] } },
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
          harnesses,
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
      assert.match(selects[0], /value="future-cli"[^>]*>Future CLI<\/option>/);
      assert.deepEqual(choices(selects[1]), providers);
      assert.doesNotMatch(selects[0], /schema-only-cli/);
    }
    const unavailable = {
      name: 'unavailable-adapter',
      graph: {
        profile: 'openengine.graph.full/v1',
        initialInput: { kind: 'null' },
        policy: {},
        root: { kind: 'seq', name: 'run', children: [] },
      },
      runtime: { harness: 'opencode', provider: 'private-provider', size: 'small', nodes: {} },
    };
    const original = structuredClone(unavailable);
    const html = renderToStaticMarkup(
      createElement(RuntimeEditor, {
        document: unavailable,
        schema,
        harnesses,
        edit: () => assert.fail('rendering must preserve unavailable authored runtime settings'),
      })
    );
    for (const id of ['opencode', 'private-provider']) {
      const option = [...html.matchAll(/<option([^>]*)>(.*?)<\/option>/g)].find((match) =>
        match[1].includes(`value="${id}"`)
      );
      assert.ok(option, `unavailable ${id} remains visible`);
      assert.match(option[1], /\bselected\b/);
      assert.match(option[1], /\bdisabled\b/);
      assert.match(option[2], /\(unavailable\)/);
    }
    assert.deepEqual(unavailable, original);
  }));

test('legacy schema choices apply only when the host catalog is absent', () =>
  withRuntimeEditor((RuntimeEditor) => {
    const legacyVariants = [
      {
        properties: {
          harness: { const: 'legacy-cli' },
          provider: { $ref: '#/$defs/LegacyProvider' },
        },
      },
      {
        properties: {
          harness: { enum: ['legacy-other'] },
          provider: { enum: ['other-service'] },
        },
      },
    ];
    const legacyDocument = {
      graph: { root: { kind: 'seq', name: 'run', children: [] } },
      runtime: { harness: 'legacy-cli', provider: 'legacy-service', size: 'small', nodes: {} },
    };
    const legacySchema = {
      oneOf: legacyVariants,
      $defs: { LegacyProvider: { enum: ['legacy-service', 'legacy-proxy'] } },
    };
    const renderLegacy = (schema: any, harnesses?: []) =>
      renderToStaticMarkup(
        createElement(RuntimeEditor, {
          document: legacyDocument,
          schema,
          harnesses,
          edit: () => assert.fail('rendering must preserve the authored runtime'),
        })
      );
    for (const schema of [
      legacySchema,
      { ...legacySchema, oneOf: undefined, anyOf: legacyVariants },
    ]) {
      const html = renderLegacy(schema);
      const selects = [...html.matchAll(/<select[^>]*>(.*?)<\/select>/g)].map((match) => match[1]);
      assert.match(selects[0], /value="legacy-cli"[^>]*>legacy-cli<\/option>/);
      assert.match(selects[0], /value="legacy-other"[^>]*>legacy-other<\/option>/);
      assert.match(selects[1], /value="legacy-service"[^>]*>legacy-service<\/option>/);
      assert.match(selects[1], /value="legacy-proxy"[^>]*>legacy-proxy<\/option>/);
      assert.doesNotMatch(html, /unavailable/);
    }
    const emptyCatalog = renderLegacy(legacySchema, []);
    assert.doesNotMatch(emptyCatalog, /legacy-other|legacy-proxy/);
    for (const id of ['legacy-cli', 'legacy-service']) {
      const option = [...emptyCatalog.matchAll(/<option([^>]*)>(.*?)<\/option>/g)].find((match) =>
        match[1].includes(`value="${id}"`)
      );
      assert.ok(option);
      assert.match(option[1], /\bselected\b/);
      assert.match(option[1], /\bdisabled\b/);
      assert.match(option[2], /\(unavailable\)/);
    }
    assert.deepEqual(legacyDocument.runtime, {
      harness: 'legacy-cli',
      provider: 'legacy-service',
      size: 'small',
      nodes: {},
    });
  }));

async function withRuntimeEditor(check: (RuntimeEditor: any) => void) {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  try {
    const { RuntimeEditor } = await server.ssrLoadModule('/src/RuntimeEditor.tsx');
    check(RuntimeEditor);
  } finally {
    await server.close();
  }
}
