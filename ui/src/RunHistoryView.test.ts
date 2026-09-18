import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import { fileURLToPath } from 'node:url';
import type { RunHistoryReader } from './run-history-source';

test('a host can render the viewer without local navigation, a run catalog, or an HTTP adapter', async (t) => {
  // ELK's browser worker is idle until a graph is selected. No layout belongs to this shell test.
  const worker = Object.getOwnPropertyDescriptor(globalThis, 'Worker');
  Object.defineProperty(globalThis, 'Worker', {
    configurable: true,
    value: class {
      postMessage() {}
      terminate() {}
    },
  });
  t.after(() => {
    if (worker) Object.defineProperty(globalThis, 'Worker', worker);
    else Reflect.deleteProperty(globalThis, 'Worker');
  });
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false, ws: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  t.after(() => server.close());
  const { RunHistoryView } = await server.ssrLoadModule('/src/RunHistoryView.tsx');
  const source: RunHistoryReader = {
    detail: async () => {
      throw new Error('Only a selected run may load history.');
    },
    page: async () => {
      throw new Error('Only a selected run may load history.');
    },
  };
  const html = renderToStaticMarkup(createElement(RunHistoryView, { source, workers: [] }));
  assert.match(html, /Run history/);
  assert.doesNotMatch(html, /app-header|run-sidebar|LOCAL|TARGET/);
});
