import { useEnvironmentCatalog } from './use-environment-catalog';
import { Field } from './Field';
import type { EnvironmentStore, EnvironmentSummary } from './environment-store';

export function EnvironmentSelector({
  value,
  store,
  onChange,
  manage,
}: {
  value?: { id: string };
  store: EnvironmentStore;
  onChange: (value?: { id: string }) => void;
  manage: () => void;
}) {
  const { items, loading, error } = useEnvironmentCatalog(store);
  const unavailable = value && !items.some((item) => item.id === value.id);
  return (
    <section aria-label="Profile environment">
      <Field label="Environment">
        <select
          value={value?.id ?? ''}
          disabled={loading}
          onChange={(event) =>
            onChange(event.target.value ? { id: event.target.value } : undefined)
          }
        >
          <option value="">Default environment</option>
          {unavailable && (
            <option value={value.id}>
              {loading ? 'Loading environment…' : 'Unavailable environment'}
            </option>
          )}
          <EnvironmentOptions items={items} />
        </select>
      </Field>
      {error && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      {!loading && unavailable && !error && (
        <p className="error-text" role="alert">
          The selected environment is unavailable in this workspace. Choose another environment.
        </p>
      )}
      <p className="helper">
        New runs use the latest saved environment. Accepted runs and resumes keep their saved
        version.
      </p>
      <button className="text-button" onClick={manage}>
        Manage environments
      </button>
    </section>
  );
}

export function EnvironmentOptions({ items }: { items: EnvironmentSummary[] }) {
  return items.map(({ id, name }) => (
    <option key={id} value={id}>
      {name}
    </option>
  ));
}
