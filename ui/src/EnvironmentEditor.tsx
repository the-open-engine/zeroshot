import { Trash2 } from 'lucide-react';
import { AddEnvironmentField } from './AddEnvironmentField';
import { ControlText } from './ControlFields';
import { Field } from './Field';
import { PreparationConnections } from './PreparationConnections';
import type { EnvironmentDefinition } from './environment-store';
import type { SeedSelection } from './file-workflows';

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
      <EnvironmentSeeds value={value} onChange={onChange} />
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

function EnvironmentSeeds({ value, onChange }: Props) {
  const seeds = value.seeds ?? [];
  function update(next: SeedSelection[]) {
    const environment: EnvironmentDefinition = { ...value, seeds: next };
    if (!next.length) delete environment.seeds;
    onChange(environment, 'seeds');
  }
  function change(index: number, seed: SeedSelection) {
    update(seeds.map((current, position) => (position === index ? seed : current)));
  }
  return (
    <section aria-label="Workspace seeds">
      <h3>Workspace seeds</h3>
      {seeds.map((seed, index) => (
        <div key={index}>
          <Field label={`Seed ${index + 1} scope`}>
            <select
              value={seed.address.scope.kind}
              onChange={(event) =>
                change(index, {
                  ...seed,
                  address: {
                    ...seed.address,
                    scope: { kind: event.target.value as 'user' | 'org' },
                  },
                })
              }
            >
              <option value="user">User library</option>
              <option value="org">Organization library</option>
            </select>
          </Field>
          <ControlText
            label={`Seed ${index + 1} path`}
            value={seed.address.path}
            onCommit={(path) => change(index, { ...seed, address: { ...seed.address, path } })}
          />
          <ControlText
            label={`Seed ${index + 1} member`}
            value={seed.member}
            onCommit={(member) => change(index, { ...seed, member })}
          />
          <button
            type="button"
            className="text-button"
            onClick={() => update(seeds.filter((_, position) => position !== index))}
          >
            <Trash2 size={14} /> Remove seed {index + 1}
          </button>
        </div>
      ))}
      <AddEnvironmentField
        onClick={() =>
          update([...seeds, { address: { scope: { kind: 'user' }, path: '' }, member: '' }])
        }
      >
        Add seed
      </AddEnvironmentField>
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
