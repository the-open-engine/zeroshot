import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { upgradePlatformWorkspaceStorage, workspaceStorageKeys } from './workspace-storage';

const workspace = { kind: 'platform' as const, id: 'cloud:user-a:org-a:user' };

function browser(t: TestContext) {
  const dom = new JSDOM('', { url: 'https://example.test/ui/' });
  const original = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', { configurable: true, value: dom.window });
  t.after(() => {
    dom.window.close();
    if (original) Object.defineProperty(globalThis, 'window', original);
    else Reflect.deleteProperty(globalThis, 'window');
  });
  return dom.window;
}

function legacyKey(mount: URL, id: string, suffix: string) {
  return `zeroshot.workspace.v1.${encodeURIComponent(
    JSON.stringify([mount.origin, new URL('.', mount).pathname, 'cloud', id])
  )}.${suffix}`;
}

test('platform upgrade recovers drafts, layouts, model choices and run position without overwriting newer state', (t) => {
  const window = browser(t);
  const mount = new URL('https://example.test/ui/');
  const keys = workspaceStorageKeys(mount, workspace);
  const entries: [Storage, string, string, string][] = [
    [window.sessionStorage, 'draft', keys.draft, '{"task":"unfinished change"}'],
    [window.sessionStorage, 'environment-draft', keys.environmentDraft, '{"setup":"prepare"}'],
    [window.sessionStorage, 'last-profile', keys.lastProfile, 'release-review'],
    [window.localStorage, 'layout.review', keys.layout('review'), '{"worker":{"x":120}}'],
    [
      window.localStorage,
      `recent-models.${encodeURIComponent(JSON.stringify(['codex', 'openai']))}`,
      keys.recentModels('codex', 'openai'),
      '["opaque/model"]',
    ],
    [
      window.sessionStorage,
      `run-view.${encodeURIComponent(JSON.stringify(['history', 'run-a']))}`,
      keys.runView('history', 'run-a'),
      '{"cursor":"v2:15","following":false}',
    ],
  ];
  for (const [storage, suffix, , value] of entries)
    storage.setItem(legacyKey(mount, workspace.id, suffix), value);
  window.sessionStorage.setItem(keys.lastProfile, 'newer-selection');

  upgradePlatformWorkspaceStorage(mount, workspace);
  for (const [storage, suffix, key, value] of entries) {
    assert.equal(storage.getItem(key), suffix === 'last-profile' ? 'newer-selection' : value);
    assert.equal(storage.getItem(legacyKey(mount, workspace.id, suffix)), null);
  }
  window.sessionStorage.setItem(keys.draft, 'newer-draft');
  upgradePlatformWorkspaceStorage(mount, workspace);
  assert.equal(window.sessionStorage.getItem(keys.draft), 'newer-draft');
  window.sessionStorage.removeItem(keys.draft);
  window.sessionStorage.setItem(legacyKey(mount, workspace.id, 'draft'), 'old-tab-draft');
  upgradePlatformWorkspaceStorage(mount, workspace);
  assert.equal(window.sessionStorage.getItem(keys.draft), null);
});

test('platform upgrade follows the explicitly identified previous UI revision for the same authority', (t) => {
  const window = browser(t);
  const previous = new URL(`https://example.test/workspace/${'a'.repeat(40)}/`);
  const current = new URL(`https://example.test/workspace/${'b'.repeat(40)}/`);
  window.sessionStorage.setItem(legacyKey(previous, workspace.id, 'draft'), 'pending-change');
  upgradePlatformWorkspaceStorage(current, workspace, previous.pathname);
  assert.equal(
    window.sessionStorage.getItem(workspaceStorageKeys(current, workspace).draft),
    'pending-change'
  );
});

test('platform browser state survives later UI asset revisions within the same authority', () => {
  const first = new URL(`https://example.test/workspace/${'a'.repeat(40)}/`);
  const next = new URL(`https://example.test/workspace/${'b'.repeat(40)}/`);
  const previousKeys = workspaceStorageKeys(first, workspace);
  const nextKeys = workspaceStorageKeys(next, workspace);
  assert.deepEqual(
    [
      previousKeys.draft,
      previousKeys.environmentDraft,
      previousKeys.lastProfile,
      previousKeys.runView('history', 'run-a'),
      previousKeys.layout('review'),
      previousKeys.recentModels('codex', 'openai'),
    ],
    [
      nextKeys.draft,
      nextKeys.environmentDraft,
      nextKeys.lastProfile,
      nextKeys.runView('history', 'run-a'),
      nextKeys.layout('review'),
      nextKeys.recentModels('codex', 'openai'),
    ]
  );
  assert.notEqual(
    workspaceStorageKeys(next, workspace).draft,
    workspaceStorageKeys(next, { ...workspace, id: 'cloud:user-b:org-a:user' }).draft
  );
});

test('platform upgrade prefers the exact mount and ignores undesignated historical revisions', (t) => {
  const window = browser(t);
  const oldest = new URL(`https://example.test/workspace/${'a'.repeat(40)}/`);
  const previous = new URL(`https://example.test/workspace/${'b'.repeat(40)}/`);
  const current = new URL(`https://example.test/workspace/${'c'.repeat(40)}/`);
  window.sessionStorage.setItem(legacyKey(oldest, workspace.id, 'draft'), 'oldest-draft');
  window.sessionStorage.setItem(legacyKey(previous, workspace.id, 'draft'), 'previous-draft');
  window.sessionStorage.setItem(legacyKey(current, workspace.id, 'draft'), 'current-mount-draft');
  upgradePlatformWorkspaceStorage(current, workspace, previous.pathname);
  const key = workspaceStorageKeys(current, workspace).draft;
  assert.equal(window.sessionStorage.getItem(key), 'current-mount-draft');
  assert.equal(window.sessionStorage.getItem(legacyKey(previous, workspace.id, 'draft')), null);
  assert.equal(
    window.sessionStorage.getItem(legacyKey(oldest, workspace.id, 'draft')),
    'oldest-draft'
  );
  window.sessionStorage.removeItem(key);
  upgradePlatformWorkspaceStorage(current, workspace, previous.pathname);
  assert.equal(window.sessionStorage.getItem(key), null);
});

test('platform upgrade cannot import another origin, mount or workspace authority', (t) => {
  const window = browser(t);
  const mount = new URL('https://example.test/ui/');
  for (const [other, id] of [
    [new URL('https://other.test/ui/'), workspace.id],
    [new URL('https://example.test/other/ui/'), workspace.id],
    [mount, 'cloud:user-b:org-a:user'],
    [mount, 'cloud:user-a:org-b:user'],
  ] as const)
    window.sessionStorage.setItem(legacyKey(other, id, 'draft'), 'foreign-draft');
  window.sessionStorage.setItem('zeroshot.workspace.v1.%INVALID%5D.draft', 'damaged-key');
  upgradePlatformWorkspaceStorage(mount, workspace);
  assert.equal(window.sessionStorage.getItem(workspaceStorageKeys(mount, workspace).draft), null);
});

test('platform upgrade handles stable environment mounts and skips local workspaces', (t) => {
  const window = browser(t);
  const mount = new URL('https://example.test/_bff/orgs/org-a/environments/');
  window.sessionStorage.setItem(legacyKey(mount, workspace.id, 'environment-draft'), 'pending-env');
  upgradePlatformWorkspaceStorage(mount, { ...workspace, kind: 'local' });
  const key = workspaceStorageKeys(mount, workspace).environmentDraft;
  assert.equal(window.sessionStorage.getItem(key), null);
  upgradePlatformWorkspaceStorage(mount, workspace);
  assert.equal(window.sessionStorage.getItem(key), 'pending-env');
});

test('blocked storage does not prevent recovery from the other browser store', (t) => {
  const window = browser(t);
  const mount = new URL('https://example.test/ui/');
  window.sessionStorage.setItem(legacyKey(mount, workspace.id, 'draft'), 'pending-change');
  Object.defineProperty(window, 'localStorage', {
    get() {
      throw new Error('storage blocked');
    },
  });
  assert.doesNotThrow(() => upgradePlatformWorkspaceStorage(mount, workspace));
  assert.equal(
    window.sessionStorage.getItem(workspaceStorageKeys(mount, workspace).draft),
    'pending-change'
  );
});

test('a full browser store preserves the old draft for a later upgrade retry', (t) => {
  const window = browser(t);
  const mount = new URL('https://example.test/ui/');
  const oldKey = legacyKey(mount, workspace.id, 'draft');
  window.sessionStorage.setItem(oldKey, 'pending-change');
  const prototype = Object.getPrototypeOf(window.sessionStorage) as Storage;
  const original = prototype.setItem;
  prototype.setItem = () => {
    throw new Error('quota exceeded');
  };
  assert.doesNotThrow(() => upgradePlatformWorkspaceStorage(mount, workspace));
  prototype.setItem = original;
  assert.equal(window.sessionStorage.getItem(oldKey), 'pending-change');
  upgradePlatformWorkspaceStorage(mount, workspace);
  assert.equal(
    window.sessionStorage.getItem(workspaceStorageKeys(mount, workspace).draft),
    'pending-change'
  );
});

test('partial quota failure cannot resurrect a migrated draft after discard or an old-tab write', (t) => {
  const window = browser(t);
  const previous = new URL(`https://example.test/workspace/${'a'.repeat(40)}/`);
  const current = new URL(`https://example.test/workspace/${'b'.repeat(40)}/`);
  const keys = workspaceStorageKeys(current, workspace);
  window.sessionStorage.setItem(legacyKey(current, workspace.id, 'draft'), 'current-draft');
  window.sessionStorage.setItem(legacyKey(previous, workspace.id, 'draft'), 'stale-draft');
  window.sessionStorage.setItem(legacyKey(current, workspace.id, 'last-profile'), 'review');
  const prototype = Object.getPrototypeOf(window.sessionStorage) as Storage;
  const original = prototype.setItem;
  t.after(() => (prototype.setItem = original));
  prototype.setItem = function (key, value) {
    if (key === keys.lastProfile) throw new Error('quota exceeded');
    original.call(this, key, value);
  };
  upgradePlatformWorkspaceStorage(current, workspace, previous.pathname);
  assert.equal(window.sessionStorage.getItem(keys.draft), 'current-draft');
  assert.equal(window.sessionStorage.getItem(keys.lastProfile), null);
  prototype.setItem = original;
  window.sessionStorage.removeItem(keys.draft);
  window.sessionStorage.setItem(legacyKey(previous, workspace.id, 'draft'), 'old-tab-write');
  upgradePlatformWorkspaceStorage(current, workspace, previous.pathname);
  assert.equal(window.sessionStorage.getItem(keys.draft), null);
  assert.equal(window.sessionStorage.getItem(keys.lastProfile), 'review');
});
