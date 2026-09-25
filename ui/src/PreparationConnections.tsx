import { Trash2 } from 'lucide-react';
import { AddEnvironmentField } from './AddEnvironmentField';
import { ControlText } from './ControlFields';

export function PreparationConnections({
  value,
  onChange,
}: {
  value: Record<string, string[]>;
  onChange: (value: Record<string, string[]>) => void;
}) {
  function rename(before: string, after: string) {
    if (!after.trim()) throw new Error('Enter the saved connection name.');
    if (before !== after && Object.hasOwn(value, after))
      throw new Error('This connection is already declared.');
    const next = { ...value };
    delete next[before];
    onChange({ ...next, [after]: value[before] });
  }
  function fields(name: string, text: string) {
    const names = text.split(/[\s,]+/).filter(Boolean);
    if (!names.length || names.some((field) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(field)))
      throw new Error('Enter environment field names separated by commas.');
    onChange({ ...value, [name]: [...new Set(names)] });
  }
  function add() {
    const keys = new Set(Object.keys(value));
    let suffix = keys.size + 1;
    while (keys.has(`connection_${suffix}`)) suffix++;
    onChange({ ...value, [`connection_${suffix}`]: ['TOKEN'] });
  }
  return (
    <section aria-label="Preparation connections">
      <h3>Preparation connections</h3>
      <p className="helper">
        Declare saved connection names and the exact secret fields setup and startup need. Manage
        their values in the target’s connection store. Agent connections stay in their node
        bindings.
      </p>
      {Object.entries(value).map(([name, names]) => (
        <div className="runtime-variable" key={name}>
          <ControlText
            label={`Preparation connection ${name} name`}
            value={name}
            onCommit={(next) => rename(name, next)}
          />
          <ControlText
            label={`Preparation connection ${name} fields`}
            value={names.join(', ')}
            onCommit={(next) => fields(name, next)}
          />
          <button
            type="button"
            className="icon-button"
            aria-label={`Remove preparation connection ${name}`}
            onClick={() => {
              const next = { ...value };
              delete next[name];
              onChange(next);
            }}
          >
            <Trash2 size={14} />
          </button>
        </div>
      ))}
      <AddEnvironmentField onClick={add}>Add preparation connection</AddEnvironmentField>
    </section>
  );
}
