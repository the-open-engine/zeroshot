import { ApiError } from './api';
import type { Document } from './domain';
import {
  acknowledgeProfileSave,
  snapshotProfileSave,
  type ProfileSaveSnapshot,
} from './profile-save';
import { flushPendingEdits, hasPendingEdits } from './pending-edits';
import type { HostCommand, WorkspaceBridge } from './workspace-bridge';

export type EditorState = {
  document?: Document;
  revision?: string;
  generation: number;
  dirty: boolean;
  pending: boolean;
  busy: boolean;
  loading: boolean;
  validation: { state: 'checking' | 'valid' | 'invalid'; message?: string };
  createProfile(templateId: string): Document;
  adopt(document: Document, saved?: Document, revision?: string): void;
  acknowledge(value: NonNullable<ReturnType<typeof acknowledgeProfileSave>>): void;
  discard(): void;
};
type PendingSave = { requestId: string; snapshot?: ProfileSaveSnapshot };

/** The mounted workspace's command lifecycle; persistence remains in its host. */
export class WorkspaceSession {
  documentId: string | null = null;
  profileDocumentId: string | null = null;
  runId?: string;
  showingRun = false;
  private pending?: PendingSave;
  constructor(
    private readonly host: WorkspaceBridge | undefined,
    private readonly read: () => EditorState,
    private readonly changed: () => void,
    private readonly fields = { flush: flushPendingEdits, pending: hasPendingEdits }
  ) {}
  get saving() {
    return this.pending !== undefined;
  }
  restore(id: string = crypto.randomUUID()) {
    this.profileDocumentId = id;
    this.documentId = id;
    this.changed();
  }
  state() {
    const editor = this.read();
    this.host?.send(
      'state',
      {
        dirty: editor.dirty,
        pending: editor.pending,
        validation: editor.validation,
        saving: this.saving,
        loading: editor.loading,
        name: editor.document?.name,
      },
      this.identity()
    );
  }
  navigate(action: 'save' | 'defaults' | 'environments') {
    this.host?.send('navigate', { action }, this.identity());
  }
  async receive(command: HostCommand) {
    try {
      this.requireDocument(command);
      await this.execute(command);
    } catch (error) {
      this.failure(command, error);
    }
  }
  private async execute(command: HostCommand) {
    switch (command.type) {
      case 'theme':
        document.documentElement.dataset.theme = command.theme;
        this.reply(command);
        return;
      case 'request_save':
        await this.snapshot(command);
        return;
      case 'save_ack':
        this.acknowledge(command);
        return;
      case 'navigate':
        this.switchSection(command);
        return;
      default:
        this.open(command);
    }
  }
  private requireDocument(command: HostCommand) {
    if (command.documentId !== this.documentId || command.generation !== this.read().generation)
      throw new ApiError(409, 'document_changed', 'The selected workspace document changed.');
  }
  private identity(requestId?: string) {
    return { documentId: this.documentId, generation: this.read().generation, requestId };
  }
  private reply(
    command: HostCommand,
    problem?: { code: string; message: string; details?: unknown }
  ) {
    this.host?.send('state', { problem, accepted: !problem }, this.identity(command.requestId));
  }
  private failure(command: HostCommand, error: unknown) {
    this.reply(command, {
      code: error instanceof ApiError ? error.code : 'workspace_error',
      message: error instanceof Error ? error.message : String(error),
      ...(error instanceof ApiError ? { details: error.details } : {}),
    });
  }
  private protect(command: HostCommand & { discard?: boolean }) {
    const state = this.read();
    if (this.pending || state.busy || state.loading)
      throw new ApiError(409, 'workspace_busy', 'Wait for the current workspace operation.');
    if ((state.dirty || state.pending || this.fields.pending()) && !command.discard)
      throw new ApiError(
        409,
        'unsaved_changes',
        'Save or discard the current draft before switching.'
      );
    if (command.discard) state.discard();
  }
  private async snapshot(command: HostCommand & { type: 'request_save' }) {
    const generation = this.read().generation;
    this.requireProfile();
    this.pending = { requestId: command.requestId };
    this.changed();
    try {
      try {
        await this.fields.flush();
      } catch (error) {
        throw new ApiError(
          409,
          'unresolved_edits',
          error instanceof Error ? error.message : 'Finish the pending field edits.'
        );
      }
      const state = this.read();
      if (!state.document || state.generation !== generation)
        throw new ApiError(
          409,
          'document_changed',
          'The profile changed before its snapshot was captured.'
        );
      const name = command.name ?? state.document.name;
      const captured = snapshotProfileSave(state.document, {
        name,
        revision: name === state.document.name ? state.revision : undefined,
        generation,
        pending: this.fields.pending() || state.pending,
      });
      this.pending = { requestId: command.requestId, snapshot: captured };
      this.host?.send(
        'save_snapshot',
        {
          profile: captured.request,
          authority: this.host.configuration?.authority,
        },
        this.identity(command.requestId)
      );
    } catch (error) {
      this.pending = undefined;
      this.changed();
      throw error;
    }
  }
  private requireProfile() {
    if (this.showingRun || !this.documentId || this.documentId !== this.profileDocumentId)
      throw new ApiError(409, 'profile_required', 'Select a profile before saving.');
    const state = this.read();
    if (this.pending || state.busy || state.loading)
      throw new ApiError(409, 'save_pending', 'Wait for the current workspace operation.');
  }
  private acknowledge(command: HostCommand & { type: 'save_ack' }) {
    const captured = this.pending;
    if (!captured?.snapshot || captured.requestId !== command.requestId)
      throw new ApiError(
        409,
        'unknown_snapshot',
        'This save acknowledgement has no pending snapshot.'
      );
    this.pending = undefined;
    this.changed();
    if (command.problem) {
      this.reply(command, command.problem);
      return;
    }
    const state = this.read();
    if (!state.document || !command.saved) return;
    const accepted = acknowledgeProfileSave(
      captured.snapshot,
      {
        generation: state.generation,
        document: state.document,
      },
      command.saved
    );
    if (accepted) state.acknowledge(accepted);
    this.reply(command);
  }
  private open(command: HostCommand & { type: 'open_profile' | 'open_run' }) {
    const next = command.type === 'open_profile' ? this.profile(command) : undefined;
    this.protect(command);
    if (command.type === 'open_profile' && next) {
      this.read().adopt(next, command.revision ? next : undefined, command.revision);
      this.profileDocumentId = command.nextDocumentId;
      this.runId = undefined;
      this.showingRun = false;
    } else if (command.type === 'open_run') {
      this.runId = command.runId;
      this.showingRun = true;
    }
    this.documentId = command.nextDocumentId;
    this.changed();
    this.reply(command);
  }
  private profile(command: HostCommand & { type: 'open_profile' }): Document {
    return command.templateId !== undefined
      ? this.read().createProfile(command.templateId)
      : command.profile;
  }
  private switchSection(command: HostCommand & { type: 'navigate' }) {
    this.protect(command);
    if (command.action === 'profiles') {
      this.documentId = this.profileDocumentId;
      this.showingRun = false;
    } else if (command.action === 'runs') this.showingRun = true;
    this.changed();
    this.reply(command);
  }
}
