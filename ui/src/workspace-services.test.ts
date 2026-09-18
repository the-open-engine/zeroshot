import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError, createApiClient, readBootstrap } from './api';
import { createWorkspaceServices } from './workspace-services';
import { workspaceStorageKeys } from './workspace-storage';
import { acknowledgeProfileSave, snapshotProfileSave } from './profile-save';
import type { Document } from './domain';

const document = (): Document => ({
  name: 'release-review',
  graph: {
    profile: 'openengine.graph.full/v1',
    initialInput: { kind: 'record', fields: {} },
    policy: {},
    root: { kind: 'seq', name: 'run', state: { kind: 'record', fields: {} }, children: [] },
  },
  runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
});
const bootstrap = () => ({
  version: 1,
  workspace: { kind: 'target', id: 'stored-authority-7' },
  templates: [],
  runtimeSchema: {},
});

test('HTTP adapter preserves conflict/authentication codes and structured error details', async () => {
  for (const status of [401, 403, 409, 410, 503]) {
    const error = {
      code: `problem_${status}`,
      message: 'Unavailable to this workspace.',
      details: { retryable: status === 503 },
    };
    const api = createApiClient(new URL('https://example.test/mount/ui/api/'), async () =>
      Response.json(error, { status })
    );
    await assert.rejects(api('profiles'), (cause: unknown) => {
      assert.ok(cause instanceof ApiError);
      assert.equal(cause.status, status);
      assert.equal(cause.code, error.code);
      assert.equal(cause.message, error.message);
      assert.deepEqual(cause.details, error.details);
      return true;
    });
  }
});

test('incompatible or unidentified workspaces fail before profile data can be restored', () => {
  const value = bootstrap();
  assert.deepEqual(readBootstrap(value), value);
  for (const version of [0, 2, '1', undefined]) {
    assert.throws(
      () => readBootstrap({ ...value, version }),
      (cause: unknown) => cause instanceof ApiError && cause.code === 'incompatible_ui'
    );
  }
  for (const workspace of [
    undefined,
    { kind: 'local' },
    { kind: 'target', id: '' },
    { kind: 'unknown', id: 'a' },
  ]) {
    assert.throws(
      () => readBootstrap({ ...value, workspace }),
      (cause: unknown) => cause instanceof ApiError && cause.code === 'invalid_bootstrap'
    );
  }
});

test('profile and history services remain on the injected mount and forward cancellation', async () => {
  const requests: { url: string; init?: RequestInit }[] = [];
  const fetcher: typeof fetch = async (url, init) => {
    requests.push({ url: String(url), init });
    if (String(url).endsWith('/runs/opaque%2Frun')) {
      const { graph, runtime } = document();
      return Response.json({
        version: 1,
        projectionVersion: 1,
        runId: 'opaque/run',
        title: 'Release review',
        phase: 'running',
        cursor: 'v2:23',
        historyAvailable: true,
        graph,
        runtime,
        initialInput: {},
        history: { initialCursor: 'v2:0', cursor: 'v2:23', complete: false, limitations: [] },
      });
    }
    return Response.json(String(url).endsWith('/bootstrap') ? bootstrap() : { profiles: [] });
  };
  const services = createWorkspaceServices(
    new URL('https://example.test/org/acme/workspace/'),
    fetcher
  );
  const controller = new AbortController();
  await services.catalog(controller.signal);
  await services.profiles.load('release/review', controller.signal);
  await services.history.detail('opaque/run', controller.signal);
  await services.history.page('opaque/run', 'v2:23', controller.signal);
  assert.deepEqual(
    requests.map(({ url }) => url),
    [
      'https://example.test/org/acme/workspace/api/bootstrap',
      'https://example.test/org/acme/workspace/api/profiles/release%2Freview',
      'https://example.test/org/acme/workspace/api/runs/opaque%2Frun',
      'https://example.test/org/acme/workspace/api/runs/opaque%2Frun/history?after=v2%3A23',
    ]
  );
  assert.ok(requests.every(({ init }) => init?.signal === controller.signal));
});

test('profile mutation uses the existing revision contract and authoring receives only content', async () => {
  const requests: { url: string; body: unknown; headers: Headers }[] = [];
  const source = document();
  const services = createWorkspaceServices(
    new URL('http://127.0.0.1:4185/ui/'),
    async (url, init) => {
      if (String(url).endsWith('/bootstrap')) return Response.json(bootstrap());
      requests.push({
        url: String(url),
        body: JSON.parse(String(init?.body)),
        headers: new Headers(init?.headers),
      });
      return Response.json({ graph: source.graph, runtime: source.runtime });
    }
  );
  await services.catalog();
  const snapshot = snapshotProfileSave(source, {
    name: source.name,
    revision: 'revision-3',
    generation: 2,
    pending: false,
  });
  await services.profiles.save(snapshot.request);
  assert.equal(requests[0].headers.get('X-Zeroshot-Workspace'), bootstrap().workspace.id);
  assert.deepEqual(requests[0].body, {
    name: source.name,
    graph: source.graph,
    runtime: source.runtime,
    expectedRevision: 'revision-3',
  });
  const updated = await services.authoring.outcome(source, { kind: 'complete', owner: 'run' });
  assert.deepEqual(requests[1].body, {
    graph: source.graph,
    runtime: source.runtime,
    action: { kind: 'complete', owner: 'run' },
  });
  assert.equal(updated.name, source.name);
  assert.equal(requests[1].headers.get('X-Zeroshot-Workspace'), null);
});

test('a tab pins its first workspace identity and never retargets an unsaved draft after replacement', async () => {
  let workspace = 'original-store';
  let catalogs = 0;
  const writes: string[] = [];
  const services = createWorkspaceServices(
    new URL('https://example.test/ui/'),
    async (url, init) => {
      if (String(url).endsWith('/bootstrap')) {
        catalogs++;
        return Response.json({ ...bootstrap(), workspace: { kind: 'local', id: workspace } });
      }
      const identity = new Headers(init?.headers).get('X-Zeroshot-Workspace');
      writes.push(identity ?? '');
      assert.notEqual(identity, workspace);
      return Response.json(
        {
          code: 'workspace_changed',
          message: 'This workspace changed. Reload before saving. Your draft is unchanged.',
        },
        { status: 409 }
      );
    }
  );
  await services.catalog();
  const draft = document();
  const original = structuredClone(draft);
  const snapshot = snapshotProfileSave(draft, { name: draft.name, generation: 1, pending: false });
  assert.equal(snapshot.request.expectedRevision, null);
  workspace = 'replacement-store';
  const changed = (error: unknown) =>
    error instanceof ApiError && error.code === 'workspace_changed' && /Reload/.test(error.message);
  await assert.rejects(services.profiles.save(snapshot.request), changed);
  assert.equal(catalogs, 1, 'A write must never reload and adopt the replacement store');
  await assert.rejects(services.catalog(), changed);
  await assert.rejects(services.profiles.save(snapshot.request), changed);
  assert.deepEqual(writes, ['original-store', 'original-store']);
  assert.deepEqual(draft, original);
  assert.deepEqual(snapshot.original, original);
});

test('profile writes cannot precede identifying their workspace', async () => {
  let requests = 0;
  const services = createWorkspaceServices(new URL('https://example.test/ui/'), async () => {
    requests++;
    return Response.json({});
  });
  const draft = document();
  await assert.rejects(
    services.profiles.save({ ...draft, expectedRevision: null }),
    (error: unknown) => error instanceof ApiError && error.code === 'workspace_required'
  );
  assert.equal(requests, 0);
});

test('draft recovery and layouts cannot cross origins, mounts, or storage identities', () => {
  const mount = new URL('https://example.test/one/ui/');
  const workspace = { kind: 'local' as const, id: 'workspace-a' };
  const first = workspaceStorageKeys(mount, workspace);
  const others = [
    workspaceStorageKeys(new URL('https://other.test/one/ui/'), workspace),
    workspaceStorageKeys(new URL('https://example.test/two/ui/'), workspace),
    workspaceStorageKeys(mount, { ...workspace, id: 'workspace-b' }),
    workspaceStorageKeys(mount, { ...workspace, kind: 'target' }),
  ];
  for (const other of others) {
    assert.notEqual(first.draft, other.draft);
    assert.notEqual(first.lastProfile, other.lastProfile);
    assert.notEqual(first.layout('shared-name'), other.layout('shared-name'));
  }
  assert.equal(
    first.draft,
    workspaceStorageKeys(new URL('https://example.test/one/ui/?theme=dark#runs'), workspace).draft
  );
});

test('save snapshots reject pending fields and isolate the exact requested content', () => {
  const source = document();
  assert.throws(
    () => snapshotProfileSave(source, { name: source.name, generation: 1, pending: true }),
    /pending field edits/
  );
  const snapshot = snapshotProfileSave(source, { name: 'copy', generation: 1, pending: false });
  assert.equal(
    snapshot.request.expectedRevision,
    null,
    'Imported/new documents never inherit a revision'
  );
  source.graph.root.name = 'changed-after-save';
  assert.equal(snapshot.request.graph.root.name, 'run');
  assert.equal(snapshot.original.graph.root.name, 'run');
});

test('acknowledging a save preserves later edits but advances their saved base', () => {
  const source = document();
  const snapshot = snapshotProfileSave(source, {
    name: 'saved-copy',
    generation: 1,
    pending: false,
  });
  const later = structuredClone(source);
  later.graph.root.name = 'newer-edit';
  const saved = {
    profile: { ...source, name: 'saved-copy', id: 'profile-1', isDefault: false },
    revision: 'revision-4',
  };
  const acknowledged = acknowledgeProfileSave(snapshot, { generation: 1, document: later }, saved)!;
  assert.equal(acknowledged.document.graph.root.name, 'newer-edit');
  assert.equal(acknowledged.document.name, 'saved-copy');
  assert.equal(acknowledged.base.graph.root.name, 'run');
  assert.equal(acknowledged.revision, 'revision-4');
  assert.equal(acknowledged.changed, true);
  assert.equal(
    acknowledgeProfileSave(snapshot, { generation: 1, document: source }, saved)?.changed,
    false
  );
});

test('an old save cannot acknowledge a newly opened document, even when its content matches', () => {
  const source = document();
  const snapshot = snapshotProfileSave(source, {
    name: source.name,
    generation: 1,
    pending: false,
  });
  const result = {
    profile: { ...source, id: 'profile-1', isDefault: false },
    revision: 'revision-2',
  };
  assert.equal(
    acknowledgeProfileSave(snapshot, { generation: 2, document: source }, result),
    undefined
  );
  assert.throws(
    () =>
      acknowledgeProfileSave(
        snapshot,
        { generation: 1, document: source },
        {
          ...result,
          profile: { ...result.profile, name: 'wrong-profile' },
        }
      ),
    /different profile/
  );
});
