import type { TestContext } from 'node:test';
import { fileURLToPath } from 'node:url';
import { createServer } from 'vite';
import type { RunDetail } from './run-history';

export async function createViteTestServer(t: TestContext) {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    configFile: false,
    server: { middlewareMode: true, watch: null, hmr: false, ws: false },
    appType: 'custom',
    optimizeDeps: { noDiscovery: true },
  });
  t.after(() => server.close());
  return server;
}

export const runDetailFixture = (runId = 'run/selected'): RunDetail => ({
  version: 1,
  projectionVersion: 1,
  runId,
  title: 'Review a change',
  phase: 'running',
  cursor: 'v2:0',
  historyAvailable: true,
  graph: {
    profile: 'openengine.graph.full/v1',
    initialInput: { kind: 'null' },
    policy: {},
    root: { kind: 'seq', name: 'run', children: [] },
  },
  runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
  initialInput: null,
  history: { initialCursor: 'v2:0', cursor: 'v2:0', complete: false, limitations: [] },
});
