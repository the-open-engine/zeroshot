import test from 'node:test';
import assert from 'node:assert/strict';
import { WorkspaceSession, type EditorState } from './workspace-session';
import { createHostedProfile } from './use-host-workspace';
import type { HostCommand, WorkspaceBridge } from './workspace-bridge';
import type { Document } from './domain';

function profile(name = 'review'): Document {
  return {
    name,
    graph: {
      profile: 'openengine.graph.full/v1',
      policy: {},
      initialInput: { kind: 'null' },
      root: { kind: 'seq', name: 'run', children: [] },
    },
    runtime: { harness: 'codex', provider: 'openai', size: 'small', nodes: {} },
  };
}
function setup() {
  const messages: Record<string, unknown>[] = [];
  let pending = false;
  const editor: EditorState = {
    document: profile(),
    revision: 'r1',
    generation: 1,
    dirty: false,
    pending: false,
    busy: false,
    loading: false,
    validation: { state: 'valid' },
    createProfile(templateId) {
      if (templateId !== 'blank' && templateId !== 'review-template')
        throw new Error('Unknown template');
      return profile(templateId === 'blank' ? '' : 'template');
    },
    adopt(document, saved, revision) {
      this.document = structuredClone(document);
      this.revision = revision;
      this.generation++;
      this.dirty = !saved;
      this.pending = false;
    },
    acknowledge(value) {
      this.document = value.document;
      this.revision = value.revision;
      this.dirty = value.changed;
    },
    discard() {
      this.dirty = false;
      this.pending = false;
      pending = false;
    },
  };
  const host = {
    configuration: { authority: { userId: 'u1', organizationId: 'o1', scope: 'user' } },
    send(type: string, payload: object, identity: object) {
      messages.push({ type, ...payload, ...identity });
    },
  } as unknown as WorkspaceBridge;
  const session = new WorkspaceSession(
    host,
    () => editor,
    () => {},
    {
      flush: async () => {
        if (pending) throw new Error('Invalid number remains unresolved.');
      },
      pending: () => pending,
    }
  );
  session.restore('profile-1');
  const command = (value: Record<string, unknown>): HostCommand =>
    ({
      version: 1,
      workspaceId: 'scope-id',
      requestId: 'request-1',
      documentId: session.documentId,
      generation: editor.generation,
      ...value,
    }) as HostCommand;
  const problem = () => (messages.at(-1)?.problem as { code?: string } | undefined)?.code;
  return {
    session,
    editor,
    messages,
    command,
    problem,
    pending(value: boolean) {
      pending = value;
      editor.pending = value;
    },
  };
}
test('later edits remain dirty when the captured save receives its acknowledgement', async () => {
  const state = setup();
  state.editor.dirty = true;
  await state.session.receive(state.command({ type: 'request_save', requestId: 'save-1' }));
  const snapshot = state.messages.at(-1)!;
  assert.equal(snapshot.type, 'save_snapshot');
  assert.deepEqual(snapshot.authority, { userId: 'u1', organizationId: 'o1', scope: 'user' });
  state.editor.document = {
    ...state.editor.document!,
    runtime: { ...state.editor.document!.runtime, provider: 'gateway' },
  };
  await state.session.receive(
    state.command({
      type: 'save_ack',
      requestId: 'save-1',
      saved: { revision: 'r2', profile: { ...profile(), id: 'id', isDefault: false } },
    })
  );
  assert.equal(state.editor.document.runtime.provider, 'gateway');
  assert.equal(state.editor.revision, 'r2');
  assert.equal(state.editor.dirty, true);
  assert.equal(state.session.saving, false);
});
test('a foreign request acknowledgement cannot settle a pending snapshot or permit a switch', async () => {
  const state = setup();
  await state.session.receive(state.command({ type: 'request_save', requestId: 'save-1' }));
  await state.session.receive(
    state.command({
      type: 'save_ack',
      requestId: 'save-2',
      saved: { revision: 'r2', profile: profile() },
    })
  );
  assert.equal(state.problem(), 'unknown_snapshot');
  assert.equal(state.session.saving, true);
  await state.session.receive(
    state.command({ type: 'open_run', nextDocumentId: 'run-doc', runId: 'run', discard: true })
  );
  assert.equal(state.problem(), 'workspace_busy');
  assert.equal(state.session.documentId, 'profile-1');
});
test('unresolved fields refuse a snapshot and retain the draft', async () => {
  const state = setup();
  state.pending(true);
  const before = structuredClone(state.editor.document);
  await state.session.receive(state.command({ type: 'request_save' }));
  assert.equal(state.problem(), 'unresolved_edits');
  assert.equal(state.session.saving, false);
  assert.deepEqual(state.editor.document, before);
  assert.ok(state.messages.every((message) => message.type !== 'save_snapshot'));
});
test('stale generations and dirty document switches are rejected before adoption', async () => {
  const state = setup();
  const open = state.command({
    type: 'open_profile',
    nextDocumentId: 'profile-2',
    profile: profile('second'),
    revision: 'r3',
  });
  await state.session.receive({ ...open, generation: 0 });
  assert.equal(state.problem(), 'document_changed');
  state.editor.dirty = true;
  await state.session.receive(open);
  assert.equal(state.problem(), 'unsaved_changes');
  assert.equal(state.editor.document!.name, 'review');
  await state.session.receive({ ...open, discard: true } as HostCommand);
  assert.equal(state.editor.document!.name, 'second');
  assert.equal(state.session.documentId, 'profile-2');
  assert.equal(state.messages.at(-1)?.generation, 2);
});
test('imported profiles start without the earlier document revision, and runs cannot save them', async () => {
  const state = setup();
  await state.session.receive(
    state.command({ type: 'open_profile', nextDocumentId: 'import', profile: profile('imported') })
  );
  await state.session.receive(state.command({ type: 'request_save', requestId: 'import-save' }));
  assert.equal(
    (state.messages.at(-1)?.profile as { expectedRevision: unknown }).expectedRevision,
    null
  );
  await state.session.receive(
    state.command({
      type: 'save_ack',
      requestId: 'import-save',
      problem: { code: 'profile_conflict', message: 'Name changed elsewhere.' },
    })
  );
  assert.equal(state.problem(), 'profile_conflict');
  assert.equal(state.editor.dirty, true);
  await state.session.receive(
    state.command({ type: 'open_run', nextDocumentId: 'run-doc', runId: 'run', discard: true })
  );
  await state.session.receive(state.command({ type: 'request_save' }));
  assert.equal(state.problem(), 'profile_required');
});

test('template opens use the editor factory and drop previous revisions', async () => {
  const state = setup();
  await state.session.receive(
    state.command({ type: 'open_profile', nextDocumentId: 'new-profile', templateId: 'blank' })
  );
  assert.equal(state.editor.document?.name, '');
  assert.equal(state.editor.revision, undefined);
  assert.equal(state.editor.dirty, true);
  assert.equal(state.session.documentId, 'new-profile');
});
test('template switches protect dirty drafts and unknown templates cannot discard them', async () => {
  const state = setup();
  state.editor.dirty = true;
  await state.session.receive(
    state.command({
      type: 'open_profile',
      nextDocumentId: 'new-profile',
      templateId: 'review-template',
    })
  );
  assert.equal(state.problem(), 'unsaved_changes');
  await state.session.receive(
    state.command({
      type: 'open_profile',
      nextDocumentId: 'new-profile',
      templateId: 'unknown',
      discard: true,
    })
  );
  assert.equal(state.session.documentId, 'profile-1');
  assert.equal(state.editor.document?.name, 'review');
  assert.equal(state.editor.dirty, true);
});

test('hosted creation selects existing blank/template factories without adopting a revision', () => {
  const template = { name: 'review', id: 'review-id', graph: profile().graph };
  const blank = createHostedProfile([template], 'blank');
  assert.deepEqual(blank.graph.initialInput, { kind: 'record', fields: {} });
  assert.deepEqual(blank.runtime.nodes, {});
  const selected = createHostedProfile([template], 'review-id');
  assert.deepEqual(selected.graph, template.graph);
  assert.notEqual(selected.graph, template.graph);
  assert.throws(() => createHostedProfile([template], 'unknown'), { code: 'unknown_template' });
});
