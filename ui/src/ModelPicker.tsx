import { useId } from 'react';
import { ChevronDown } from 'lucide-react';
import { suggestedModels } from './models';
export function ModelPicker({
  label,
  value,
  harness,
  provider,
  onChange,
  openRuntime,
  compact = false,
}: {
  label: string;
  value: string;
  harness: string;
  provider: string;
  onChange: (value: string) => void;
  openRuntime?: () => void;
  compact?: boolean;
}) {
  const id = useId(),
    options = suggestedModels(harness, provider),
    missing = [!harness && 'harness', !provider && 'provider'].filter(Boolean).join(' and '),
    showRuntimeHint = !!missing && !!openRuntime;
  return (
    <div className={compact ? 'model-picker compact-picker' : 'field model-picker'}>
      <label htmlFor={id} className={compact ? 'sr-only' : ''}>
        {label}
      </label>
      <div className="model-control">
        <input
          id={id}
          value={value}
          placeholder="Model ID"
          aria-describedby={showRuntimeHint ? `${id}-runtime-hint` : undefined}
          onChange={(e) => onChange(e.target.value)}
        />
        {!!options.length && (
          <div className="model-presets">
            <ChevronDown size={15} aria-hidden="true" />
            <select
              value=""
              aria-label={`Suggested models for ${label === 'Model' ? 'this node' : label.replace('Model for ', '')}`}
              title="Suggested models"
              onChange={(e) => {
                if (e.target.value) onChange(e.target.value);
              }}
            >
              <option value="" disabled>
                Suggested models
              </option>
              {options.map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
          </div>
        )}
      </div>
      {showRuntimeHint && (
        <p id={`${id}-runtime-hint`} className="model-runtime-hint">
          Choose a {missing} in{' '}
          <button type="button" aria-haspopup="dialog" onClick={openRuntime}>
            Runtime settings
          </button>
        </p>
      )}
    </div>
  );
}
