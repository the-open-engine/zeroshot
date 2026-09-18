import { useSyncExternalStore } from 'react';
import { Field } from './Field';
import { numericValue } from './numeric-edit';
import { useNumericDrafts, type NumericField } from './numeric-drafts';

export function NumberField({
  label,
  node,
  field,
  value,
  onChange,
  optional = false,
  placeholder,
}: {
  label: string;
  node: string;
  field: NumericField;
  value: number | null | undefined;
  onChange: (value: number | undefined) => void;
  optional?: boolean;
  placeholder?: string;
}) {
  const text = value == null ? '' : String(value);
  const drafts = useNumericDrafts();
  useSyncExternalStore(drafts.subscribe, drafts.version, drafts.version);
  const draft = drafts.get(node, field)?.text ?? text;
  const parsed = numericValue({ text: draft, badInput: false }, optional);
  function commit() {
    drafts.commit(node, field, optional, onChange);
  }
  return (
    <Field label={label}>
      <input
        type="text"
        inputMode="decimal"
        value={draft}
        placeholder={placeholder}
        aria-invalid={!parsed.valid}
        onChange={(event) => drafts.set({ node, field, text: event.target.value, base: value })}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault();
            commit();
          }
          if (event.key === 'Escape') {
            event.preventDefault();
            drafts.clear(node, field);
          }
        }}
      />
      {!parsed.valid && (
        <small className="error-text" role="alert">
          Enter a whole number of at least 1.
        </small>
      )}
    </Field>
  );
}
