import { useSyncExternalStore } from 'react';
import { Field } from './Field';
import { numericFieldValue, useNumericDrafts, type NumericField } from './numeric-drafts';

export function NumberField({
  label,
  node,
  field,
  value,
  onChange,
  optional = false,
  placeholder,
  invalid,
}: {
  label: string;
  node?: string;
  field: NumericField;
  value: number | null | undefined;
  onChange: (value: number | undefined) => void;
  optional?: boolean;
  placeholder?: string;
  invalid?: boolean;
}) {
  const text = value == null ? '' : String(value);
  const drafts = useNumericDrafts();
  useSyncExternalStore(drafts.subscribe, drafts.version, drafts.version);
  const draft = drafts.get(node, field)?.text ?? text;
  const probability = field === 'minProbability';
  const parsed = numericFieldValue(field, draft, optional);
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
        aria-invalid={!parsed.valid || invalid || undefined}
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
          {probability
            ? 'Enter a number from 0 to 1.'
            : field === 'maxConcurrency'
              ? 'Enter a whole number from 1 to 65,536.'
              : 'Enter a whole number of at least 1.'}
        </small>
      )}
    </Field>
  );
}
