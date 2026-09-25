import test from 'node:test';
import assert from 'node:assert/strict';
import { createWorkspaceServices } from './workspace-services';

test('environment CRUD shares workspace identity and conditional writes across hosts', async () => {
  const seen: { path: string; init?: RequestInit }[] = [];
  const services = createWorkspaceServices(
    new URL('https://example.test/workspace/'),
    async (url, init) => {
      seen.push({ path: String(url), init });
      if (String(url).endsWith('bootstrap'))
        return Response.json({
          version: 1,
          workspace: { kind: 'local', id: 'store-1' },
          templates: [],
          runtimeSchema: {},
        });
      return Response.json({ deleted: true, environments: [] });
    }
  );
  await assert.rejects(
    services.environments.save({ name: 'env', definition: {}, expectedRevision: null }),
    /identify/
  );
  assert.equal(seen.length, 0);
  await services.catalog();
  const controller = new AbortController();
  await services.environments.list(controller.signal);
  await services.environments.load('id/with/slash', controller.signal);
  await services.environments.save(
    { name: 'env', definition: { startup: 'npm ci' }, expectedRevision: null },
    controller.signal
  );
  await services.environments.remove('env-1', 'revision-2', controller.signal);
  assert.equal(seen[2].path, 'https://example.test/workspace/api/environments/id%2Fwith%2Fslash');
  for (const request of seen.slice(3)) {
    assert.equal(new Headers(request.init?.headers).get('X-Zeroshot-Workspace'), 'store-1');
    assert.equal(request.init?.signal, controller.signal);
  }
  assert.equal(seen[4].init?.method, 'DELETE');
  assert.deepEqual(JSON.parse(String(seen[4].init?.body)), { expectedRevision: 'revision-2' });
});
