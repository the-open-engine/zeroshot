import { AppHeader } from './AppHeader';
import { EnvironmentEditor } from './EnvironmentEditor';
import { Field } from './Field';
import { workspaceStorageKeys } from './workspace-storage';
import { EnvironmentOptions } from './EnvironmentSelector';
import { useEnvironmentWorkspace } from './use-environment-workspace';
import { useEnvironmentHost } from './environment-workspace-host';
import type { WorkspaceServices } from './workspace-services';
import type { Bootstrap } from './api';
import type { WorkspaceBridge } from './workspace-bridge';

type Props = {
  services: WorkspaceServices;
  bootstrap: Bootstrap;
  host?: WorkspaceBridge;
  readOnly?: boolean;
};
export function EnvironmentWorkspace({ services, bootstrap, host, readOnly = false }: Props) {
  const draftKey = workspaceStorageKeys(services.mount, bootstrap.workspace).environmentDraft;
  const model = useEnvironmentWorkspace(services.environments, readOnly, draftKey);
  useEnvironmentHost(
    host,
    {
      dirty: model.dirty,
      pending: model.pending,
      busy: model.busy,
      loading: model.loading,
      name: model.draft?.name,
    },
    model.discard
  );
  return (
    <div className={`app environment-app ${host ? 'embedded-app' : ''}`}>
      {!host && (
        <AppHeader
          section="environments"
          workspace={bootstrap.workspace}
          navigate={(section) =>
            model.guard(() => {
              window.location.hash = section === 'profiles' ? '' : section;
            })
          }
        />
      )}
      <main className="environment-workspace">
        <EnvironmentSelection model={model} readOnly={readOnly} />
        {model.loading && <p role="status">Loading environments…</p>}
        {model.error && (
          <p className="error-text" role="alert">
            {model.error}
          </p>
        )}
        {model.notice && <p role="status">{model.notice}</p>}
        {readOnly && <p className="helper">This environment is read-only.</p>}
        {model.draft && <EnvironmentForm model={model} readOnly={readOnly} />}
      </main>
    </div>
  );
}
function EnvironmentForm({
  model,
  readOnly,
}: {
  model: ReturnType<typeof useEnvironmentWorkspace>;
  readOnly: boolean;
}) {
  const draft = model.draft!;
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        model.save();
      }}
    >
      <fieldset className="environment-fields" disabled={readOnly || model.busy}>
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
      <p className="helper">
        Saving updates every referencing profile for new runs. Accepted runs and resumes keep their
        saved environment.
      </p>
      <div className="environment-toolbar">
        {!readOnly && (
          <button className="button primary" type="submit" disabled={model.busy}>
            {model.busy ? 'Saving…' : 'Save environment'}
          </button>
        )}
        {model.saved && (
          <button
            className="button"
            type="button"
            disabled={model.busy}
            onClick={() => model.open(model.saved!.id)}
          >
            Reload environment
          </button>
        )}
        {!readOnly && model.saved && (
          <button className="button" type="button" disabled={model.busy} onClick={model.remove}>
            Delete environment
          </button>
        )}
        <span role="status">
          {model.pending ? 'Unfinished fields' : model.dirty ? 'Unsaved changes' : ''}
        </span>
      </div>
    </form>
  );
}

function EnvironmentSelection({
  model,
  readOnly,
}: {
  model: ReturnType<typeof useEnvironmentWorkspace>;
  readOnly: boolean;
}) {
  return (
    <div className="environment-toolbar">
      <Field label="Select environment">
        <select
          value={model.draft?.id ?? ''}
          disabled={model.loading || model.busy}
          onChange={(event) => {
            if (event.target.value) model.open(event.target.value);
          }}
        >
          <option value="">{model.draft ? 'Unsaved environment' : 'Select an environment'}</option>
          <EnvironmentOptions items={model.items} />
        </select>
      </Field>
      {!readOnly && (
        <button className="button" disabled={model.loading || model.busy} onClick={model.create}>
          New environment
        </button>
      )}
    </div>
  );
}
