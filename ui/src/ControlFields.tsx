import { useEffect, useId, useState } from 'react';
import { usePendingText } from './pending-edits';
import { numericValue } from './numeric-edit';

export function ControlText({
  label,
  value,
  onCommit,
  numeric = false,
}: {
  label: string;
  value: string;
  onCommit: (value: string) => void;
  numeric?: boolean;
}) {
  const [draft, setDraft] = useState(value);
  const [error, setError] = useState('');
  const [badInput, setBadInput] = useState(false);
  const errorId = useId();
  const unresolved = numeric && !numericValue({ text: draft, badInput }).valid;
  usePendingText(draft !== value || unresolved);
  useEffect(() => {
    setDraft(value);
    setError('');
    setBadInput(false);
  }, [value]);
  function commit() {
    if (unresolved) {
      setError('Enter a whole number of at least 1.');
      return;
    }
    if (draft === value) return;
    try {
      onCommit(draft);
      setError('');
    } catch (error) {
      setError((error as Error).message);
    }
  }
  return (
    <div className="control-text">
      <input
        aria-label={label}
        aria-invalid={!!error || unresolved}
        aria-describedby={error ? errorId : undefined}
        value={draft}
        type={numeric ? 'number' : 'text'}
        min={numeric ? 1 : undefined}
        max={numeric ? Number.MAX_SAFE_INTEGER : undefined}
        step={numeric ? 1 : undefined}
        onInput={(event) => {
          setDraft(event.currentTarget.value);
          setBadInput(numeric && event.currentTarget.validity.badInput);
        }}
        onChange={(event) => {
          setDraft(event.target.value);
          setBadInput(numeric && event.target.validity.badInput);
        }}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault();
            commit();
          }
          if (event.key === 'Escape') {
            event.currentTarget.value = value;
            setDraft(value);
            setBadInput(false);
            setError('');
          }
        }}
      />
      {error && (
        <p className="error-text" id={errorId} role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
