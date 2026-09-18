import { useEffect, useRef, useState } from 'react';
import { Braces, Plus, Trash2 } from 'lucide-react';
import { useAuthoring } from './AuthoringProvider';
import { allNodes, executable, type Document, type GraphNode } from './domain';
import { usePendingText } from './pending-edits';
import {
  editableFieldType,
  freshType,
  recordFields,
  typeLabels,
  typeSummary,
  type Payload,
} from './schema';
import {
  editOutputField,
  inputSourceId,
  inputSourceLabel,
  mapSourceLabel,
  nodeDataChoices,
  nodeInputRows,
  nodeOutputFields,
  removeOutputField,
  renameInputField,
  uniqueDataField,
  type NodeDataAction,
  type NodeOutputField,
} from './node-data';
import './node-data.css';

/** Two input events may arrive before React paints the disabled state. */
export async function withDataEditLock(
  pending: { current: boolean },
  action: () => Promise<void>
): Promise<void> {
  if (pending.current)
    throw new Error(
      'The previous data edit is still applying. Wait for it to finish, then retry this edit.'
    );
  pending.current = true;
  try {
    await action();
  } finally {
    pending.current = false;
  }
}

export function NodeDataEditor({
  document,
  node,
  edit,
  openJson,
  selectNode,
}: {
  document: Document;
  node: GraphNode;
  edit: (document: Document) => void;
  openJson: (target: string) => void;
  selectNode: (name: string) => void;
}) {
  const authoring = useAuthoring();
  const root = node.name === document.graph.root.name;
  const fixed = document.runtime.nodes[node.name]?.kind === 'git_delivery';
  const [busy, setBusy] = useState(false),
    [error, setError] = useState('');
  usePendingText(busy);
  const current = useRef(document),
    mounted = useRef(true),
    pending = useRef(false);
  current.current = document;
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  function local(apply: () => Document) {
    try {
      edit(apply());
      setError('');
    } catch (cause) {
      setError((cause as Error).message);
    }
  }
  async function change(action: NodeDataAction) {
    try {
      await withDataEditLock(pending, async () => {
        setBusy(true);
        setError('');
        const before = JSON.stringify(document);
        try {
          const next = await authoring.data(document, action);
          if (!mounted.current) return;
          if (JSON.stringify(current.current) !== before)
            throw new Error('The profile changed. Apply this edit again.');
          edit(next);
        } finally {
          if (mounted.current) setBusy(false);
        }
      });
    } catch (cause) {
      if (mounted.current) setError((cause as Error).message);
    }
  }
  const rootFields = Object.entries(recordFields(document.graph.initialInput) ?? {}).map(
    ([name, field]) => ({ name, ...field })
  );
  const outputs = nodeOutputFields(node);
  return (
    <div
      className="node-data-editor"
      aria-busy={busy}
      onKeyDown={(event) => event.key === 'Escape' && setError('')}
    >
      <fieldset disabled={busy} className="node-data-fields">
        {root && (
          <DataSection title="Inputs" raw={() => openJson('graph')}>
            {rootFields.map((field) => (
              <DeclaredField
                key={field.name}
                field={{ ...field, channel: 'out' }}
                label="Input"
                onChange={(next) =>
                  void change({
                    kind: 'run_input_field',
                    before: field.name,
                    name: next.name,
                    type: next.type,
                    required: next.required,
                  })
                }
                remove={() => void change({ kind: 'remove_run_input', name: field.name })}
                raw={() => openJson('graph')}
              />
            ))}
            {!recordFields(document.graph.initialInput) &&
              document.graph.initialInput?.kind !== 'null' && (
                <span className="node-data-type">{typeSummary(document.graph.initialInput)}</span>
              )}
            {(recordFields(document.graph.initialInput) ||
              document.graph.initialInput?.kind === 'null') && (
              <button
                className="text-button"
                onClick={() =>
                  void change({
                    kind: 'run_input_field',
                    name: uniqueDataField(rootFields),
                    type: { kind: 'string' },
                    required: true,
                  })
                }
              >
                <Plus size={14} /> Add input
              </button>
            )}
          </DataSection>
        )}
        {executable(node) && (
          <BoundFields
            title="Inputs"
            document={document}
            node={node}
            change={change}
            rename={(before, after) =>
              local(() => renameInputField(document, node.name, before, after))
            }
            raw={() => openJson(node.name)}
            fixed={fixed}
            busy={busy}
          />
        )}
        {node.kind === 'map' && (
          <DataSection title="Inputs" raw={() => openJson(node.name)}>
            <label className="node-data-source-label">
              Collection
              <CollectionSelect document={document} node={node} change={change} busy={busy} />
            </label>
          </DataSection>
        )}
        {node.kind === 'succeed' && (
          <BoundFields
            title="Outputs"
            document={document}
            node={node}
            change={change}
            rename={(before, after) =>
              local(() => renameInputField(document, node.name, before, after))
            }
            raw={() => openJson(node.name)}
            busy={busy}
          />
        )}
        {executable(node) && (
          <DataSection title="Outputs" raw={() => openJson(node.name)}>
            {outputs.map((field) => (
              <DeclaredField
                key={`${field.channel}:${field.name}`}
                field={field}
                label={field.channel === 'signal' ? 'Outcome' : 'Output'}
                fixed={fixed}
                onChange={(next) => local(() => editOutputField(document, node.name, field, next))}
                remove={() => local(() => removeOutputField(document, node.name, field))}
                raw={() => openJson(node.name)}
              />
            ))}
            {['output', ...(node.kind === 'verifier' ? ['diagnostic'] : [])].map((channel) =>
              !recordFields(node[channel]) && node[channel]?.kind !== 'null' && node[channel] ? (
                <span key={channel} className="node-data-type">
                  {typeSummary(node[channel])}
                </span>
              ) : null
            )}
            {!fixed &&
              (recordFields(node.output) ||
                node.output?.kind === 'null' ||
                node.output === undefined) && (
                <div className="node-data-add-actions">
                  <button
                    className="text-button"
                    onClick={() =>
                      local(() =>
                        editOutputField(document, node.name, undefined, {
                          name: uniqueDataField(outputs, 'result'),
                          channel: 'out',
                          type: { kind: 'string' },
                          required: true,
                        })
                      )
                    }
                  >
                    <Plus size={14} /> Add output
                  </button>
                  {node.kind === 'verifier' && (
                    <button
                      className="text-button"
                      onClick={() =>
                        local(() =>
                          editOutputField(document, node.name, undefined, {
                            name: uniqueDataField(outputs, 'verdict'),
                            channel: 'signal',
                            type: { kind: 'enum', values: ['accepted', 'rejected'] },
                            required: true,
                          })
                        )
                      }
                    >
                      <Plus size={14} /> Add outcome
                    </button>
                  )}
                </div>
              )}
          </DataSection>
        )}
        {!root && ['seq', 'choice', 'par', 'loop', 'map'].includes(node.kind) && (
          <DataSection title="Outputs" raw={() => openJson(node.name)}>
            {allNodes(node)
              .filter(executable)
              .flatMap((child) =>
                nodeOutputFields(child).map((field) => (
                  <button
                    key={`${child.name}:${field.channel}:${field.name}`}
                    className="node-data-child"
                    onClick={() => selectNode(child.name)}
                  >
                    <span>
                      {child.name} · {field.name}
                    </span>
                    <small>
                      {field.channel === 'signal' ? 'Outcome' : typeSummary(field.type)}
                    </small>
                  </button>
                ))
              )}
          </DataSection>
        )}
      </fieldset>
      {busy && (
        <p className="node-data-type" role="status">
          Applying changes…
        </p>
      )}
      {error && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function DataSection({
  title,
  raw,
  children,
}: {
  title: string;
  raw: () => void;
  children: React.ReactNode;
}) {
  return (
    <section className="node-data-section" aria-label={title}>
      <header>
        <h3>{title}</h3>
        <button
          className="icon-button"
          title={`${title} JSON`}
          aria-label={`${title} JSON`}
          onClick={raw}
        >
          <Braces size={14} />
        </button>
      </header>
      {children}
    </section>
  );
}

export function BoundFields({
  title,
  document,
  node,
  change,
  rename,
  raw,
  fixed = false,
  busy = false,
}: {
  title: 'Inputs' | 'Outputs';
  document: Document;
  node: GraphNode;
  change: (action: NodeDataAction) => Promise<void>;
  rename: (before: string, after: string) => void;
  raw: () => void;
  fixed?: boolean;
  busy?: boolean;
}) {
  const choices = nodeDataChoices(document, node),
    rows = nodeInputRows(node);
  const input = title === 'Inputs';
  const schema = node.kind === 'succeed' ? node.output : node.input;
  return (
    <DataSection title={title} raw={raw}>
      {rows.map((field) => {
        const selected = inputSourceId(document, node, field.name, choices);
        return (
          <div className="node-data-bound" key={field.name}>
            <div className="node-data-bound-heading">
              <DataText
                label={`${input ? 'Input' : 'Output'} ${field.name} name`}
                value={field.name}
                commit={(value) => rename(field.name, value)}
                disabled={fixed || busy}
              />
              <span className="node-data-type">{typeSummary(field.type)}</span>
              {!fixed && (
                <button
                  className="icon-button"
                  title={`Remove ${field.name}`}
                  aria-label={`Remove ${input ? 'input' : 'output'} ${field.name}`}
                  disabled={busy}
                  onClick={() =>
                    void change({
                      kind: 'remove_input',
                      target: { node: node.name, input: field.name },
                    })
                  }
                >
                  <Trash2 size={14} />
                </button>
              )}
            </div>
            <select
              aria-label={`Source for ${field.name}`}
              disabled={busy}
              value={selected}
              onChange={(event) => {
                const choice = choices.find((entry) => entry.id === event.target.value);
                if (choice)
                  void change({
                    kind: 'connect',
                    target: { node: node.name, input: field.name },
                    source: choice.source,
                  });
              }}
            >
              <option value="">Choose source</option>
              {selected && !choices.some((choice) => choice.id === selected) && (
                <option value={selected}>{inputSourceLabel(document, node, field.name)}</option>
              )}
              {choices.map((choice) => (
                <option key={choice.id} value={choice.id}>
                  {choice.label}
                </option>
              ))}
            </select>
          </div>
        );
      })}
      {!recordFields(schema) && schema?.kind !== 'null' && (
        <span className="node-data-type">{typeSummary(schema)}</span>
      )}
      {!fixed && (recordFields(schema) || schema?.kind === 'null' || schema === undefined) && (
        <select
          className="node-data-add-source"
          aria-label={`Add ${input ? 'input' : 'output'} from`}
          disabled={busy}
          value=""
          onChange={(event) => {
            const choice = choices.find((entry) => entry.id === event.target.value);
            if (choice)
              void change({
                kind: 'connect',
                target: {
                  node: node.name,
                  input: uniqueDataField(rows, choice.source.path.at(-1) ?? 'result'),
                },
                source: choice.source,
              });
          }}
        >
          <option value="">+ Add {input ? 'input' : 'output'}</option>
          {choices.map((choice) => (
            <option key={choice.id} value={choice.id}>
              {choice.label}
            </option>
          ))}
        </select>
      )}
    </DataSection>
  );
}

function CollectionSelect({
  document,
  node,
  change,
  busy,
}: {
  document: Document;
  node: GraphNode;
  change: (action: NodeDataAction) => Promise<void>;
  busy: boolean;
}) {
  const choices = nodeDataChoices(document, node).filter((choice) => choice.type.kind === 'array');
  const label = mapSourceLabel(document, node);
  const selected =
    label === 'Choose collection'
      ? ''
      : (choices.find((choice) => choice.label === label)?.id ?? (node.over ? 'saved' : ''));
  return (
    <select
      aria-label="Collection"
      disabled={busy}
      value={selected}
      onChange={(event) => {
        const choice = choices.find((entry) => entry.id === event.target.value);
        if (choice) void change({ kind: 'map_collection', node: node.name, source: choice.source });
      }}
    >
      <option value="">Choose collection</option>
      {selected === 'saved' && <option value="saved">{label}</option>}
      {choices.map((choice) => (
        <option key={choice.id} value={choice.id}>
          {choice.label}
        </option>
      ))}
    </select>
  );
}

function DeclaredField({
  field,
  label,
  onChange,
  remove,
  raw,
  fixed = false,
}: {
  field: NodeOutputField;
  label: string;
  onChange: (next: NodeOutputField) => void;
  remove: () => void;
  raw: () => void;
  fixed?: boolean;
}) {
  return (
    <div className="node-data-declaration">
      <div className="node-data-declaration-heading">
        <DataText
          label={`${label} ${field.name} name`}
          value={field.name}
          commit={(name) => onChange({ ...field, name })}
          disabled={fixed}
        />
        {field.channel === 'signal' ? (
          <span className="node-data-type">Outcome</span>
        ) : (
          <TypeSelect
            label={`${label} ${field.name} type`}
            value={field.type}
            onChange={(type) => onChange({ ...field, type })}
            disabled={fixed}
            raw={raw}
          />
        )}
        {!fixed && (
          <button
            className="icon-button"
            aria-label={`Remove ${label.toLowerCase()} ${field.name}`}
            title={`Remove ${field.name}`}
            onClick={remove}
          >
            <Trash2 size={14} />
          </button>
        )}
      </div>
      {field.channel !== 'signal' && (
        <label className="node-data-required">
          <input
            aria-label={`${label} ${field.name} required`}
            type="checkbox"
            checked={field.required}
            disabled={fixed}
            onChange={(event) => onChange({ ...field, required: event.target.checked })}
          />
          Required
        </label>
      )}
      <TypeDetails
        label={`${label} ${field.name}`}
        type={field.type}
        change={(type) => onChange({ ...field, type })}
        raw={raw}
        disabled={fixed}
      />
    </div>
  );
}

function DataText({
  label,
  value,
  commit,
  disabled = false,
}: {
  label: string;
  value: string;
  commit: (value: string) => void;
  disabled?: boolean;
}) {
  const [draft, setDraft] = useState(value);
  usePendingText(draft !== value);
  useEffect(() => setDraft(value), [value]);
  const apply = () => {
    if (draft !== value) commit(draft);
  };
  return (
    <input
      aria-label={label}
      value={draft}
      disabled={disabled}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={apply}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          apply();
        }
        if (event.key === 'Escape') setDraft(value);
      }}
    />
  );
}

function TypeSelect({
  label,
  value,
  onChange,
  disabled,
  raw,
}: {
  label: string;
  value: Payload;
  onChange: (value: Payload) => void;
  disabled?: boolean;
  raw: () => void;
}) {
  if (!editableFieldType(value))
    return (
      <button className="text-button node-data-type" onClick={raw}>
        {typeSummary(value)} <Braces size={12} />
      </button>
    );
  return (
    <select
      aria-label={label}
      value={value.kind}
      disabled={disabled}
      onChange={(event) => onChange(freshType(event.target.value))}
    >
      {['string', 'boolean', 'integer', 'number', 'enum', 'array', 'null'].map((kind) => (
        <option key={kind} value={kind}>
          {typeLabels[kind]}
        </option>
      ))}
    </select>
  );
}

function TypeDetails({
  label,
  type,
  change,
  raw,
  disabled = false,
}: {
  label: string;
  type: Payload;
  change: (value: Payload) => void;
  raw: () => void;
  disabled?: boolean;
}) {
  if (!type || typeof type !== 'object') return null;
  if (
    type.kind === 'enum' &&
    Array.isArray(type.values) &&
    type.values.every((value: unknown) => typeof value === 'string')
  )
    return <EnumOptions label={label} type={type} change={change} disabled={disabled} />;
  if (type.kind !== 'array' || !type.items) return null;
  const fields = recordFields(type.items);
  return (
    <div className="node-data-items">
      <label>
        Items
        <select
          aria-label={`${label} item type`}
          value={type.items.kind}
          disabled={disabled}
          onChange={(event) => change({ ...type, items: freshType(event.target.value) })}
        >
          {!['string', 'boolean', 'integer', 'number', 'record', 'null'].includes(
            type.items.kind
          ) && <option value={type.items.kind}>{typeSummary(type.items)}</option>}
          {['string', 'boolean', 'integer', 'number', 'record', 'null'].map((kind) => (
            <option key={kind} value={kind}>
              {typeLabels[kind]}
            </option>
          ))}
        </select>
      </label>
      {fields && (
        <div className="node-data-item-fields">
          {Object.entries(fields).map(([name, field]) => (
            <DeclaredField
              key={name}
              label={`${label} item`}
              field={{ name, ...field, channel: 'out' }}
              fixed={disabled}
              raw={raw}
              onChange={(next) => {
                const nextFields = { ...fields };
                if (next.name !== name && Object.hasOwn(nextFields, next.name)) return;
                delete nextFields[name];
                nextFields[next.name] = { ...field, type: next.type, required: next.required };
                change({ ...type, items: { ...type.items, fields: nextFields } });
              }}
              remove={() =>
                change({
                  ...type,
                  items: {
                    ...type.items,
                    fields: Object.fromEntries(
                      Object.entries(fields).filter(([key]) => key !== name)
                    ),
                  },
                })
              }
            />
          ))}
          {!disabled && (
            <button
              className="text-button"
              onClick={() => {
                const name = uniqueDataField(Object.keys(fields).map((name) => ({ name })));
                change({
                  ...type,
                  items: {
                    ...type.items,
                    fields: { ...fields, [name]: { type: { kind: 'string' }, required: true } },
                  },
                });
              }}
            >
              <Plus size={13} /> Add item field
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function EnumOptions({
  label,
  type,
  change,
  disabled,
}: {
  label: string;
  type: Payload;
  change: (value: Payload) => void;
  disabled: boolean;
}) {
  const value: string = type.values.join(', ');
  const [draft, setDraft] = useState(value),
    [error, setError] = useState('');
  usePendingText(draft !== value);
  useEffect(() => setDraft(value), [value]);
  function commit() {
    if (draft === value) return;
    const values = draft
      .split(/[\n,]/)
      .map((value) => value.trim())
      .filter(Boolean);
    if (
      !values.length ||
      new Set(values).size !== values.length ||
      values.some((value) => !/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(value))
    ) {
      setError('Use unique option names.');
      return;
    }
    change({ ...type, values });
    setError('');
  }
  return (
    <label className="node-data-options">
      Options
      <input
        aria-label={`${label} options`}
        value={draft}
        disabled={disabled}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === 'Enter') commit();
          if (event.key === 'Escape') {
            setDraft(value);
            setError('');
          }
        }}
      />
      {error && (
        <span className="error-text" role="alert">
          {error}
        </span>
      )}
    </label>
  );
}
