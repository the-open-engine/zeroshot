import { useEffect, useId, useState, type KeyboardEvent } from 'react';
import { ChevronDown } from 'lucide-react';
import type { ModelGroup } from './models';

/**
 * A model ID text field with an optional list of earlier choices. Any typed ID is the value;
 * suggestions only fill the field, so the control looks the same with or without them.
 */
export function ModelPicker({
  label,
  value,
  suggestions,
  harness,
  provider,
  onChange,
  openRuntime,
  compact = false,
  invalid,
  placeholder = 'Model ID',
}: {
  label: string;
  value: string;
  suggestions: ModelGroup[];
  harness: string;
  provider: string;
  onChange: (value: string) => void;
  openRuntime?: () => void;
  compact?: boolean;
  invalid?: boolean;
  placeholder?: string;
}) {
  const id = useId(),
    listId = `${id}-list`,
    // `typed` filters the list once the user types; opening it alone shows every suggestion.
    [open, setOpen] = useState(false),
    [typed, setTyped] = useState(false),
    [active, setActive] = useState(-1),
    groups = matching(suggestions, typed ? value : ''),
    options = groups.flatMap((group) => group.models),
    expanded = open && options.length > 0,
    missing = [!harness && 'harness', !provider && 'provider'].filter(Boolean).join(' and '),
    showRuntimeHint = !!missing && !!openRuntime,
    hintId = showRuntimeHint ? `${id}-runtime-hint` : undefined,
    optionId = (index: number) => `${id}-option-${index}`;
  useEffect(() => {
    // Keep the keyboard choice visible in a list taller than its box.
    if (expanded && active >= 0)
      document.getElementById(optionId(active))?.scrollIntoView?.({ block: 'nearest' });
  }, [expanded, active]);
  function show() {
    setOpen(true);
    setTyped(false);
    setActive(-1);
  }
  function choose(model: string) {
    onChange(model);
    setOpen(false);
  }
  function keyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const all = expanded ? options : matching(suggestions, '').flatMap((g) => g.models);
      if (!all.length) return;
      if (!expanded) show();
      const step = e.key === 'ArrowDown' ? 1 : -1;
      // With nothing highlighted yet, ArrowDown starts at the first option and ArrowUp at the last.
      setActive((index) =>
        expanded && index >= 0
          ? (index + step + all.length) % all.length
          : step === 1
            ? 0
            : all.length - 1
      );
    } else if (e.key === 'Enter' && expanded && active >= 0) {
      e.preventDefault();
      choose(options[active]);
    } else if (e.key === 'Escape' && expanded) {
      // Close the list without also closing the dialog around it.
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    }
  }
  let index = -1;
  return (
    <div className={compact ? 'model-picker compact-picker' : 'field model-picker'}>
      <label htmlFor={id} className={compact ? 'visually-hidden' : undefined}>
        {label}
      </label>
      <div className="model-combobox">
        <input
          id={id}
          role="combobox"
          value={value}
          placeholder={placeholder}
          autoComplete="off"
          spellCheck={false}
          aria-autocomplete="list"
          aria-expanded={expanded}
          aria-controls={expanded ? listId : undefined}
          aria-activedescendant={expanded && active >= 0 ? optionId(active) : undefined}
          aria-invalid={invalid || undefined}
          aria-describedby={hintId}
          onFocus={show}
          onClick={() => !open && show()}
          onBlur={() => setOpen(false)}
          onKeyDown={keyDown}
          onChange={(e) => {
            setOpen(true);
            setTyped(true);
            setActive(-1);
            onChange(e.target.value);
          }}
        />
        {suggestions.length > 0 && (
          <button
            type="button"
            className="model-toggle"
            tabIndex={-1}
            aria-hidden="true"
            onMouseDown={(e) => {
              // Keep focus in the input so the list follows it.
              e.preventDefault();
              const input = e.currentTarget.previousElementSibling as HTMLInputElement | null;
              if (document.activeElement !== input) input?.focus();
              else if (expanded) setOpen(false);
              else show();
            }}
          >
            <ChevronDown size={15} />
          </button>
        )}
        {expanded && (
          <ul
            id={listId}
            role="listbox"
            aria-label={`Suggestions for ${label}`}
            className="model-options"
          >
            {groups.map((group) => (
              <li key={group.label} role="presentation">
                <div className="model-group-label" aria-hidden="true">
                  {group.label}
                </div>
                <ul role="group" aria-label={group.label}>
                  {group.models.map((model) => {
                    const at = ++index;
                    return (
                      <li
                        key={model}
                        id={optionId(at)}
                        role="option"
                        aria-selected={model === value}
                        className={at === active ? 'active' : undefined}
                        onMouseDown={(e) => e.preventDefault()}
                        onMouseEnter={() => setActive(at)}
                        onClick={() => choose(model)}
                      >
                        {model}
                      </li>
                    );
                  })}
                </ul>
              </li>
            ))}
          </ul>
        )}
      </div>
      {showRuntimeHint && (
        <p id={hintId} className="model-runtime-hint">
          Choose a {missing} in{' '}
          <button type="button" aria-haspopup="dialog" onClick={openRuntime}>
            Runtime settings
          </button>
        </p>
      )}
    </div>
  );
}

/** Groups narrowed to models containing `query`, without repeating a model or the query itself. */
function matching(groups: ModelGroup[], query: string): ModelGroup[] {
  const seen = new Set<string>(),
    needle = query.trim().toLowerCase();
  return groups
    .map((group) => ({
      label: group.label,
      models: group.models.filter((model) => {
        if (seen.has(model)) return false;
        seen.add(model);
        return !needle || (model !== query && model.toLowerCase().includes(needle));
      }),
    }))
    .filter((group) => group.models.length);
}
