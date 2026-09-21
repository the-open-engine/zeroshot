import test from 'node:test';
import assert from 'node:assert/strict';
import { WorkspaceBridge, type HostCommand, type WorkspaceInit } from './workspace-bridge';
function setup() {
  const sent: Record<string, unknown>[] = [],
    origins: string[] = [];
  let listener!: (event: Pick<MessageEvent, 'source' | 'origin' | 'data'>) => void;
  let disposed = false;
  const parent = {
    postMessage(message: unknown, origin: string) {
      sent.push(message as Record<string, unknown>);
      origins.push(origin);
    },
  };
  const configurations: WorkspaceInit[] = [],
    commands: HostCommand[] = [];
  const bridge = new WorkspaceBridge(
    'https://cloud.example',
    parent,
    (handler) => {
      listener = handler;
      return () => {
        disposed = true;
      };
    },
    (value) => configurations.push(value)
  );
  bridge.connect((value) => commands.push(value));
  const emit = (data: unknown, source: unknown = parent, origin = 'https://cloud.example') =>
    listener({ data, source: source as MessageEventSource, origin });
  const init = {
    version: 1,
    type: 'init',
    requestId: bridge.readyId,
    workspaceId: 'user-org-scope-id',
    authority: { userId: 'user', organizationId: 'org', scope: 'user' },
    apiBase: '/_bff/orgs/org/workspace/user/',
    theme: 'dark',
  };
  return {
    bridge,
    sent,
    origins,
    configurations,
    commands,
    emit,
    init,
    get disposed() {
      return disposed;
    },
  };
}
test('init requires the exact parent, origin and ready identity', () => {
  const state = setup();
  state.emit(state.init, {});
  state.emit(state.init, undefined, 'https://untrusted.example');
  state.emit({ ...state.init, requestId: 'old-ready' });
  assert.equal(state.configurations.length, 0);
  state.emit(state.init);
  assert.equal(state.configurations.length, 1);
  state.emit({ ...state.init, workspaceId: 'new-authority' });
  assert.equal(state.bridge.configuration?.workspaceId, state.init.workspaceId);
  state.bridge.dispose();
  assert.equal(state.disposed, true);
  assert.ok(state.origins.every((value) => value === 'https://cloud.example'));
});
test('foreign service paths and incomplete authority scopes fail init', () => {
  for (const apiBase of [
    '//foreign.example/api/',
    '/api/?token=secret',
    '/api/#fragment',
    '/api',
    '//[invalid/',
  ]) {
    const state = setup();
    state.emit({ ...state.init, apiBase });
    assert.equal(state.configurations.length, 0);
  }
  for (const authority of [
    { userId: 'user', scope: 'user' },
    { userId: 'user', organizationId: 'org', scope: 'other' },
  ]) {
    const state = setup();
    state.emit({ ...state.init, authority });
    assert.equal(state.configurations.length, 0);
  }
});
test('commands require workspace/document/request identities and replies retain them', () => {
  const state = setup();
  state.emit(state.init);
  const open = {
    version: 1,
    type: 'open_run',
    requestId: 'open-1',
    workspaceId: state.init.workspaceId,
    documentId: null,
    generation: 0,
    nextDocumentId: 'run-document',
    runId: 'selected-run',
  };
  state.emit({ ...open, workspaceId: 'another-user' });
  state.emit({ ...open, requestId: '' });
  state.emit({ ...open, documentId: undefined });
  state.emit({ ...open, generation: undefined });
  state.emit({ ...open, generation: -1 });
  assert.equal(state.commands.length, 0);
  state.emit(open);
  assert.deepEqual(state.commands, [open]);
  state.bridge.send(
    'state',
    { dirty: true },
    { documentId: 'run-document', generation: 1, requestId: 'open-1' }
  );
  assert.deepEqual(state.sent.at(-1), {
    version: 1,
    type: 'state',
    workspaceId: state.init.workspaceId,
    documentId: 'run-document',
    generation: 1,
    requestId: 'open-1',
    dirty: true,
  });
});
test('save denials keep structured codes and malformed acknowledgements are rejected', () => {
  const state = setup();
  state.emit(state.init);
  const ack = {
    version: 1,
    type: 'save_ack',
    workspaceId: state.init.workspaceId,
    documentId: 'profile-document',
    generation: 1,
    requestId: 'save-3',
  };
  state.emit({ ...ack, saved: { revision: 'next', profile: {} } });
  assert.equal(state.commands.length, 0);
  const denied = {
    ...ack,
    problem: {
      code: 'profile_conflict',
      message: 'Reload this profile.',
      details: { revision: 'new' },
    },
  };
  state.emit(denied);
  assert.deepEqual(state.commands, [denied]);
});

test('disposed bridges ignore incoming commands and cannot send host state', () => {
  const state = setup();
  state.emit(state.init);
  state.bridge.dispose();
  const sent = state.sent.length;
  state.bridge.send('state', {}, { documentId: null, generation: 0 });
  state.emit({
    version: 1,
    type: 'theme',
    workspaceId: state.init.workspaceId,
    documentId: null,
    generation: 0,
    requestId: 'disposed',
    theme: 'dark',
  });
  assert.equal(state.sent.length, sent);
  assert.equal(state.commands.length, 0);
});
