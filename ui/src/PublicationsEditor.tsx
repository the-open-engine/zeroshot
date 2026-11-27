import { Trash2 } from 'lucide-react';
import { ControlText } from './ControlFields';
import { bindingContext, dataChoices, payloadAtPath } from './bindings';
import type { Document, GraphNode } from './domain';
import { Field } from './Field';

/** Native admission proves filepath compatibility and availability for bound publications. */
export function PublicationsEditor({
  document,
  node,
  onChange,
}: {
  document: Document;
  node: GraphNode;
  onChange: (node: GraphNode) => void;
}) {
  const artifacts = node.artifacts ?? {};
  const context = bindingContext(document, node);
  const choices = dataChoices(context).filter(
    ({ value }) =>
      payloadAtPath(context[value.source as 'state' | 'item'], value.path)?.kind === 'filepath'
  );
  function update(next: Record<string, any>) {
    const replacement: GraphNode = { ...node, artifacts: next };
    if (!Object.keys(next).length) delete replacement.artifacts;
    onChange(replacement);
  }
  function change(name: string, value: any) {
    update({ ...artifacts, [name]: value });
  }
  function add() {
    let name = 'file';
    for (let suffix = 2; Object.hasOwn(artifacts, name); suffix++) name = `file_${suffix}`;
    change(name, { kind: 'literal', path: '' });
  }
  return (
    <section className="control-section" aria-label="Published files">
      <h3>{node.kind === 'succeed' ? 'Published files' : 'Request attachments'}</h3>
      {Object.entries(artifacts).map(([name, entry]: [string, any]) => (
        <div key={name}>
          <ControlText
            label={`Publication ${name} name`}
            value={name}
            onCommit={(next) => {
              if (!/^[A-Za-z_][A-Za-z0-9_.-]{0,127}$/.test(next))
                throw new Error('Use a valid field name.');
              if (next !== name && Object.hasOwn(artifacts, next))
                throw new Error('This name already exists.');
              update(
                Object.fromEntries(
                  Object.entries(artifacts).map(([key, value]) => [
                    key === name ? next : key,
                    value,
                  ])
                )
              );
            }}
          />
          <Field label={`${name} source`}>
            <select
              value={entry.kind === 'binding' ? JSON.stringify(entry.value) : 'literal'}
              onChange={(event) =>
                change(
                  name,
                  event.target.value === 'literal'
                    ? { kind: 'literal', path: '' }
                    : { kind: 'binding', value: JSON.parse(event.target.value) }
                )
              }
            >
              <option value="literal">Workspace path</option>
              {entry.kind === 'binding' &&
                !choices.some(
                  ({ value }) => JSON.stringify(value) === JSON.stringify(entry.value)
                ) && (
                  <option value={JSON.stringify(entry.value)}>Saved binding (unavailable)</option>
                )}
              {choices.map(({ label, value }) => (
                <option key={JSON.stringify(value)} value={JSON.stringify(value)}>
                  {label}
                </option>
              ))}
            </select>
          </Field>
          {entry.kind === 'literal' && (
            <ControlText
              label={`${name} path`}
              value={entry.path}
              onCommit={(path) => change(name, { ...entry, path })}
            />
          )}
          <button
            type="button"
            className="text-button"
            onClick={() =>
              update(Object.fromEntries(Object.entries(artifacts).filter(([key]) => key !== name)))
            }
          >
            <Trash2 size={14} /> Remove {name}
          </button>
        </div>
      ))}
      <button type="button" className="text-button" onClick={add}>
        Add file or folder
      </button>
    </section>
  );
}
