import test from 'node:test';
import assert from 'node:assert/strict';
import { createEnvironmentStore } from './environment-store';
import { createWorkspaceServices } from './workspace-services';

test('hosted environment reads project host responses and preserve conditional-write identity', async () => {
  const seen: { path: string; init?: RequestInit }[] = [];
  const environment = {
    id: 'env-1',
    name: 'node-tools',
    revision: 'r1',
    definition: { startup: 'npm ci' },
  };
  const store = createEnvironmentStore(
    new URL('https://example.test/workspace/environments?context=repo-1'),
    'store-1',
    async (url, init) => {
      seen.push({ path: String(url), init });
      if (init?.method === 'DELETE') return Response.json({ deleted: true });
      if (String(url).includes('id%2Fwith%2Fslash'))
        return Response.json({ ...environment, hostMetadata: { private: true } });
      return Response.json({
        environments: [{ ...environment, hostMetadata: { private: true } }],
      });
    }
  );
  const controller = new AbortController();
  assert.deepEqual(await store.list(controller.signal), {
    environments: [{ id: environment.id, name: environment.name, revision: environment.revision }],
  });
  assert.deepEqual(await store.load('id/with/slash', controller.signal), environment);
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
  assert.equal(seen[2].init?.method, 'DELETE');
  assert.deepEqual(JSON.parse(String(seen[2].init?.body)), { expectedRevision: 'revision-2' });
});

test('plain local workspace services do not expose environment storage', () => {
  const services = createWorkspaceServices(new URL('https://example.test/ui/'));
  assert.equal('environments' in services, false);
});
