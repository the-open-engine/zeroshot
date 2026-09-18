import { useEffect, useState } from 'react';
import { Braces, Plus, Trash2 } from 'lucide-react';
import {
  addField,
  editableFieldType,
  fieldUpdate,
  freshType,
  recordFields,
  removeField,
  renameField,
  scalarKinds,
  typeLabels,
  typeSummary,
  type Payload,
} from './schema';
import './schema.css';
import { usePendingText } from './pending-edits';

export function SchemaEditor({
  label,
  value,
  onChange,
  openJson,
  copyFrom,
  copyLabel,
}: {
  label: string;
  value: Payload;
  onChange: (value: Payload) => void;
  openJson: () => void;
  copyFrom?: Payload;
  copyLabel?: string;
}) {
  const fields = recordFields(value);
  const supported = editableFieldType(value) || !!fields;
  const [pending, setPending] = useState('');
  useEffect(() => setPending(''), [value]);
  return (
    <details className="schema-editor">
      <summary>
        <span>{label}</span>
        <span className="schema-summary" title={typeSummary(value)}>
          {typeSummary(value)}
        </span>
      </summary>
      <div className="schema-content">
        {copyFrom && (
          <button
            className="text-button"
            onClick={() => {
              if (fields && Object.keys(fields).length) setPending('copy');
              else onChange(structuredClone(copyFrom));
            }}
          >
            {copyLabel ?? 'Copy schema'}
          </button>
        )}
        <label className="schema-type-label">
          Type
          <select
            aria-label={`${label} type`}
            value={supported ? value.kind : 'advanced'}
            onChange={(e) => {
              if (e.target.value === value?.kind) return;
              if (!!fields || !supported || value?.kind === 'enum' || value?.kind === 'array')
                setPending(e.target.value);
              else onChange(freshType(e.target.value));
            }}
          >
            {!supported && <option value="advanced">Advanced schema</option>}
            {Object.entries(typeLabels).map(([kind, text]) => (
              <option key={kind} value={kind}>
                {text}
              </option>
            ))}
          </select>
        </label>
        {pending && (
          <div className="schema-confirm" role="alert">
            <p>
              Replace this schema with{' '}
              {pending === 'copy' ? 'the copied schema' : typeLabels[pending].toLowerCase()}? Its
              current fields and type settings will be removed.
            </p>
            <div>
              <button className="button compact" onClick={() => setPending('')}>
                Keep schema
              </button>
              <button
                className="button compact"
                onClick={() => {
                  onChange(pending === 'copy' ? structuredClone(copyFrom!) : freshType(pending));
                  setPending('');
                }}
              >
                Replace schema
              </button>
            </div>
          </div>
        )}
        {fields ? (
          <ObjectFields label={label} value={value} onChange={onChange} openJson={openJson} />
        ) : supported ? (
          <TypeSettings label={label} value={value} onChange={onChange} openJson={openJson} />
        ) : (
          <button className="text-button" onClick={openJson}>
            <Braces size={14} />
            Edit advanced schema in JSON
          </button>
        )}
      </div>
    </details>
  );
}
function ObjectFields({
  label,
  value,
  onChange,
  openJson,
  item = false,
}: {
  label: string;
  value: Payload;
  onChange: (value: Payload) => void;
  openJson: () => void;
  item?: boolean;
}) {
  return (
    <>
      {Object.entries(recordFields(value) ?? {}).map(([name, field]) => (
        <SchemaField
          key={name}
          label={label}
          name={name}
          field={field}
          rename={(next) => onChange(renameField(value, name, next))}
          update={(patch) => onChange(fieldUpdate(value, name, patch))}
          remove={() => onChange(removeField(value, name))}
          openJson={openJson}
          allowObjectItems={!item}
        />
      ))}
      <button className="text-button schema-add" onClick={() => onChange(addField(value))}>
        <Plus size={14} />
        {item ? 'Add item field' : 'Add field'}
      </button>
    </>
  );
}
function SchemaField({
  label,
  name,
  field,
  rename,
  update,
  remove,
  openJson,
  allowObjectItems = true,
}: {
  label: string;
  name: string;
  field: any;
  rename: (name: string) => void;
  update: (patch: Record<string, any>) => void;
  remove: () => void;
  openJson: () => void;
  allowObjectItems?: boolean;
}) {
  const [draft, setDraft] = useState(name),
    [error, setError] = useState(''),
    [pendingType, setPendingType] = useState('');
  usePendingText(draft !== name);
  const supported = editableFieldType(field?.type, allowObjectItems);
  function commit() {
    if (draft === name) return;
    try {
      rename(draft);
      setError('');
    } catch (e) {
      setDraft(name);
      setError(`Name unchanged. ${(e as Error).message}`);
    }
  }
  return (
    <div className="schema-field">
      <div className="schema-field-row">
        <input
          aria-label={`${label} field ${name} name`}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === 'Enter') commit();
            if (e.key === 'Escape') {
              setDraft(name);
              setError('');
            }
          }}
        />
        <select
          aria-label={`${label} field ${name} type`}
          value={supported ? field.type.kind : 'advanced'}
          disabled={!supported}
          onChange={(e) => {
            if (e.target.value === field.type.kind) return;
            if (recordFields(field.type?.items)) setPendingType(e.target.value);
            else update({ type: freshType(e.target.value) });
          }}
        >
          {!supported && <option value="advanced">Nested / JSON</option>}
          {[...scalarKinds, 'enum', 'array'].map((kind) => (
            <option key={kind} value={kind}>
              {typeLabels[kind]}
            </option>
          ))}
        </select>
        <button
          className="icon-button"
          title={`Remove ${name}`}
          aria-label={`Remove ${label.toLowerCase()} field ${name}`}
          onClick={remove}
        >
          <Trash2 size={14} />
        </button>
      </div>
      {pendingType && (
        <TypeReplacement
          description="Replace this field type? Its item fields will be removed."
          cancel={() => setPendingType('')}
          replace={() => {
            update({ type: freshType(pendingType) });
            setPendingType('');
          }}
        />
      )}
      <div className="schema-field-options">
        <label>
          <input
            type="checkbox"
            aria-label={`${label} field ${name} required`}
            checked={field?.required === true}
            onChange={(e) => update({ required: e.target.checked })}
          />
          Required
        </label>
        {!supported && (
          <button className="text-button" onClick={openJson}>
            Edit JSON
          </button>
        )}
      </div>
      {error && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      {supported && (
        <TypeSettings
          label={`${label} field ${name}`}
          value={field.type}
          onChange={(type) => update({ type })}
          openJson={openJson}
          allowObjectItems={allowObjectItems}
        />
      )}
    </div>
  );
}
function TypeSettings({
  label,
  value,
  onChange,
  openJson,
  allowObjectItems = true,
}: {
  label: string;
  value: Payload;
  onChange: (v: Payload) => void;
  openJson: () => void;
  allowObjectItems?: boolean;
}) {
  if (value.kind === 'array')
    return <ArrayItems {...{ label, value, onChange, openJson, allowObjectItems }} />;
  if (value.kind === 'enum') return <EnumValues label={label} value={value} onChange={onChange} />;
  return null;
}
function ArrayItems({
  label,
  value,
  onChange,
  openJson,
  allowObjectItems,
}: {
  label: string;
  value: Payload;
  onChange: (value: Payload) => void;
  openJson: () => void;
  allowObjectItems: boolean;
}) {
  const [pendingType, setPendingType] = useState('');
  const fields = allowObjectItems && recordFields(value.items);
  return (
    <>
      <label className="schema-type-label">
        Items
        <select
          aria-label={`${label} item type`}
          value={value.items.kind}
          onChange={(e) => {
            if (e.target.value === value.items.kind) return;
            if (fields) setPendingType(e.target.value);
            else onChange({ ...value, items: freshType(e.target.value) });
          }}
        >
          {[...scalarKinds, ...(allowObjectItems ? ['record'] : [])].map((kind) => (
            <option key={kind} value={kind}>
              {typeLabels[kind]}
            </option>
          ))}
        </select>
      </label>
      {pendingType && (
        <TypeReplacement
          description="Replace the item type? Its current fields will be removed."
          cancel={() => setPendingType('')}
          replace={() => {
            onChange({ ...value, items: freshType(pendingType) });
            setPendingType('');
          }}
        />
      )}
      {fields && (
        <div className="schema-item-fields" role="group" aria-label={`${label} item fields`}>
          <span className="schema-item-heading">Each item</span>
          <ObjectFields
            label={`${label} item`}
            value={value.items}
            onChange={(items) => onChange({ ...value, items })}
            openJson={openJson}
            item
          />
        </div>
      )}
    </>
  );
}
function TypeReplacement({
  description,
  cancel,
  replace,
}: {
  description: string;
  cancel: () => void;
  replace: () => void;
}) {
  return (
    <div className="schema-confirm" role="alert">
      <p>{description}</p>
      <div>
        <button className="button compact" onClick={cancel}>
          Keep type
        </button>
        <button className="button compact" onClick={replace}>
          Replace type
        </button>
      </div>
    </div>
  );
}
function EnumValues({
  label,
  value,
  onChange,
}: {
  label: string;
  value: Payload;
  onChange: (v: Payload) => void;
}) {
  const text: string = value.values.join('\n');
  const [draft, setDraft] = useState(text),
    [error, setError] = useState('');
  usePendingText(draft !== text);
  useEffect(() => {
    setDraft(text);
    setError('');
  }, [text]);
  function commit() {
    if (draft === text) return;
    const values = draft
      .split('\n')
      .map((s) => s.trim())
      .filter(Boolean);
    if (
      !values.length ||
      new Set(values).size !== values.length ||
      values.some((s) => !/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(s) || s.length > 128)
    ) {
      setDraft(text);
      setError(
        'Values unchanged. Use unique labels, one per line, starting with a letter or underscore.'
      );
      return;
    }
    onChange({ ...value, values });
    setError('');
  }
  return (
    <label className="schema-enum-label">
      Values
      <textarea
        aria-label={`${label} enum values`}
        rows={3}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
      />
      {error && (
        <span className="error-text" role="alert">
          {error}
        </span>
      )}
    </label>
  );
}
