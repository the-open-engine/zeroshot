import test from 'node:test';
import assert from 'node:assert/strict';
import { createEnvironmentStore } from './environment-store';
import { createWorkspaceServices } from './workspace-services';

test('hosted environment CRUD preserves explicit context and conditional-write identity', async () => {
  const seen: { path: string; init?: RequestInit }[] = [];
  const store = createEnvironmentStore(
    new URL('https://example.test/workspace/environments?context=repo-1'),
    'store-1',
    async (url, init) => {
      seen.push({ path: String(url), init });
      return Response.json({ deleted: true, environments: [] });
    }
  );
  const controller = new AbortController();
  await store.list(controller.signal);
  await store.load('id/with/slash', controller.signal);
  await store.save(
    { name: 'env', definition: { startup: 'npm ci' }, expectedRevision: null },
    controller.signal
  );
  await store.remove('env-1', 'revision-2', controller.signal);
  assert.equal(
    seen[1].path,
    'https://example.test/workspace/environments/id%2Fwith%2Fslash?context=repo-1'
  );
  for (const request of seen) assert.equal(new URL(request.path).search, '?context=repo-1');
  for (const request of seen.slice(2)) {
    assert.equal(new Headers(request.init?.headers).get('X-Zeroshot-Workspace'), 'store-1');
    assert.equal(request.init?.signal, controller.signal);
  }
  assert.equal(seen[3].init?.method, 'DELETE');
  assert.deepEqual(JSON.parse(String(seen[3].init?.body)), { expectedRevision: 'revision-2' });
});

test('plain local workspace services do not expose environment storage', () => {
  const services = createWorkspaceServices(new URL('https://example.test/ui/'));
  assert.equal('environments' in services, false);
});
