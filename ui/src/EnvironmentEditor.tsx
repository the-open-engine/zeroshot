import { Trash2 } from 'lucide-react';
import { AddEnvironmentField } from './AddEnvironmentField';
import { ControlText } from './ControlFields';
import { Field } from './Field';
import { PreparationConnections } from './PreparationConnections';
import type { EnvironmentDefinition } from './environment-store';

type Props = {
  value: EnvironmentDefinition;
  onChange: (value: EnvironmentDefinition, key: string) => void;
};

export function EnvironmentEditor({ value, onChange }: Props) {
  function scriptChange(key: 'setup' | 'startup', script: string) {
    const next = { ...value, [key]: script };
    if (!script) delete next[key];
    onChange(next, key);
  }
  return (
    <section aria-label="Run environment">
      <div className="section-title">Environment</div>
      <Field label="Setup script">
        <textarea
          className="mono"
          rows={5}
          spellCheck={false}
          value={value.setup ?? ''}
          placeholder="apt-get update && apt-get install -y build-essential"
          onChange={(event) => scriptChange('setup', event.target.value)}
        />
      </Field>
      <Field label="Startup script">
        <textarea
          className="mono"
          rows={5}
          spellCheck={false}
          value={value.startup ?? ''}
          placeholder="npm ci"
          onChange={(event) => scriptChange('startup', event.target.value)}
        />
      </Field>
      <EnvironmentVariables value={value} onChange={onChange} />
      <div className="section-rule" />
      <PreparationConnections
        value={value.connections ?? {}}
        onChange={(connections) => {
          const next: EnvironmentDefinition = { ...value, connections };
          if (!Object.keys(connections).length) delete next.connections;
          onChange(next, 'connections');
        }}
      />
    </section>
  );
}

function EnvironmentVariables({ value, onChange }: Props) {
  const variables = value.variables ?? {};
  function update(next: Record<string, string>) {
    const environment: EnvironmentDefinition = { ...value, variables: next };
    if (!Object.keys(next).length) delete environment.variables;
    onChange(environment, 'variables');
  }
  function rename(before: string, after: string) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(after))
      throw new Error('Use a letter or underscore, followed by letters, digits or underscores.');
    if (before !== after && Object.hasOwn(variables, after))
      throw new Error('This variable already exists.');
    update(
      Object.fromEntries(
        Object.entries(variables).map(([name, entry]) => [name === before ? after : name, entry])
      )
    );
  }
  function add() {
    let name = 'VARIABLE';
    for (let suffix = 2; Object.hasOwn(variables, name); suffix++) name = `VARIABLE_${suffix}`;
    update({ ...variables, [name]: '' });
  }
  return (
    <div aria-label="Environment variables">
      {Object.entries(variables).map(([name, entry]) => (
        <div className="runtime-variable" key={name}>
          <ControlText
            label={`Variable ${name} name`}
            value={name}
            onCommit={(next) => rename(name, next)}
          />
          <input
            aria-label={`Variable ${name} value`}
            value={entry}
            onChange={(event) => update({ ...variables, [name]: event.target.value })}
          />
          <button
            type="button"
            className="icon-button"
            aria-label={`Remove variable ${name}`}
            onClick={() =>
              update(Object.fromEntries(Object.entries(variables).filter(([key]) => key !== name)))
            }
          >
            <Trash2 size={14} />
          </button>
        </div>
      ))}
      <AddEnvironmentField onClick={add}>Add variable</AddEnvironmentField>
    </div>
  );
}
