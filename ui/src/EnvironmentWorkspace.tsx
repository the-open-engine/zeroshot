import { useRef } from 'react';
import { EnvironmentEditor } from './EnvironmentEditor';
import { Field } from './Field';
import { workspaceStorageKeys } from './workspace-storage';
import { EnvironmentOptions } from './EnvironmentOptions';
import { useEnvironmentWorkspace } from './use-environment-workspace';
import { useEnvironmentHost } from './environment-workspace-host';
import type { EnvironmentStore } from './environment-store';
import type { Bootstrap } from './api';
import type { WorkspaceBridge } from './workspace-bridge';

type Props = {
  store: EnvironmentStore;
  collection: URL;
  bootstrap: Bootstrap;
  host: WorkspaceBridge;
  readOnly?: boolean;
};
export function EnvironmentWorkspace({
  store,
  collection,
  bootstrap,
  host,
  readOnly = false,
}: Props) {
  const settingsDirty = useRef(false);
  const draftKey = workspaceStorageKeys(collection, bootstrap.workspace).environmentDraft;
  const model = useEnvironmentWorkspace(
    store,
    readOnly,
    collection.search ? `${draftKey}:${collection.search}` : draftKey,
    () => settingsDirty.current
  );
  const saving = useEnvironmentHost(
    host,
    {
      dirty: model.dirty,
      pending: model.pending,
      busy: model.busy,
      loading: model.loading,
      hasDraft: !!model.draft,
      name: model.draft?.name,
      resourceId: model.saved?.id ?? null,
      resourceRevision: model.saved?.revision ?? null,
    },
    { discard: model.discard, capture: model.capture, acknowledge: model.acknowledge },
    settingsDirty,
    readOnly
  );
  return (
    <div className="app environment-app embedded-app">
      <main className="environment-workspace">
        <EnvironmentSelection model={model} readOnly={readOnly} saving={saving} />
        {model.loading && <p role="status">Loading environments…</p>}
        {model.error && (
          <p className="error-text" role="alert">
            {model.error}
          </p>
        )}
        {model.notice && <p role="status">{model.notice}</p>}
        {model.draft && (
          <EnvironmentForm
            key={model.editorGeneration}
            model={model}
            readOnly={readOnly}
            saving={saving}
          />
        )}
      </main>
    </div>
  );
}
function EnvironmentForm({
  model,
  readOnly,
  saving,
}: {
  model: ReturnType<typeof useEnvironmentWorkspace>;
  readOnly: boolean;
  saving: boolean;
}) {
  const draft = model.draft!;
  return (
    <div>
      <fieldset className="environment-fields" disabled={readOnly || model.busy || saving}>
        <Field label="Environment name">
          <input
            value={draft.name}
            required
            maxLength={64}
            pattern="[A-Za-z0-9][A-Za-z0-9._-]{0,63}"
            onChange={(event) => model.change({ ...draft, name: event.target.value })}
          />
        </Field>
        <EnvironmentEditor
          value={draft.definition}
          onChange={(definition) => model.change({ ...draft, definition })}
        />
      </fieldset>
      {!readOnly && model.saved && (
        <div className="environment-toolbar">
          <button
            className="button"
            type="button"
            disabled={model.busy || saving}
            onClick={model.remove}
          >
            Delete environment
          </button>
        </div>
      )}
    </div>
  );
}

function EnvironmentSelection({
  model,
  readOnly,
  saving,
}: {
  model: ReturnType<typeof useEnvironmentWorkspace>;
  readOnly: boolean;
  saving: boolean;
}) {
  return (
    <div className="environment-toolbar">
      <Field label="Select environment">
        <select
          value={model.draft?.id ?? ''}
          disabled={model.loading || model.busy || saving}
          onChange={(event) => {
            if (event.target.value) model.open(event.target.value);
          }}
        >
          <option value="">{model.draft ? 'Unsaved environment' : 'Select an environment'}</option>
          <EnvironmentOptions items={model.items} />
        </select>
      </Field>
      {!readOnly && (
        <button
          className="button"
          disabled={model.loading || model.busy || saving}
          onClick={model.create}
        >
          New environment
        </button>
      )}
    </div>
  );
}
