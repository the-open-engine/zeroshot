import { serve } from '@hono/node-server';
import { Hono } from 'hono';
import { pathToFileURL } from 'node:url';
import { runDeepHealthCheck } from './health.js';

export const app = new Hono();

app.get('/health/deep', (c) => {
  const health = runDeepHealthCheck();

  if (!health.ok) {
    return c.json(health, 503);
  }

  return c.json(health);
});

export function startServer(
  port = Number.parseInt(process.env.PORT ?? '8787', 10),
) {
  const server = serve({
    fetch: app.fetch,
    port,
  });

  console.log(`beta API listening on http://localhost:${port}`);

  return server;
}

const entryPoint = process.argv[1]
  ? pathToFileURL(process.argv[1]).href
  : undefined;

if (import.meta.url === entryPoint) {
  startServer();
}
