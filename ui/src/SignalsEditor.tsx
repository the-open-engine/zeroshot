import { Plus, Trash2 } from 'lucide-react';
import { ControlText } from './ControlFields';
import {
  addSignal,
  appendSignalLabel,
  changeSignalLabel,
  isObject,
  isStringList,
  renameSignal,
  type Signals,
} from './guards';
import './guards.css';

export function SignalsEditor({
  value,
  onChange,
}: {
  value: Signals;
  onChange: (value: Signals) => void;
}) {
  if (!isObject(value))
    return (
      <section className="control-section">
        <p className="helper">
          This signal declaration is preserved unchanged. Replace it to define signals visually.
        </p>
        <button className="button compact" onClick={() => onChange(addSignal({}))}>
          Replace with verdict signal
        </button>
      </section>
    );
  return (
    <section className="control-section signals-editor" aria-label="Verifier signals">
      <h3>Signals</h3>
      <p className="helper">
        Named results that conditions can match. A verdict commonly allows accepted or rejected.
      </p>
      {Object.entries(value).map(([name, labels]) => (
        <fieldset className="signal-declaration" key={name}>
          <legend>{name}</legend>
          <div className="signal-name-row">
            <ControlText
              label={`Signal ${name} name`}
              value={name}
              onCommit={(next) => onChange(renameSignal(value, name, next))}
            />
            <button
              className="icon-button control-remove"
              title={`Remove signal ${name}`}
              aria-label={`Remove signal ${name}`}
              onClick={() =>
                onChange(Object.fromEntries(Object.entries(value).filter(([key]) => key !== name)))
              }
            >
              <Trash2 size={14} />
            </button>
          </div>
          {isStringList(labels) ? (
            <>
              <div className="signal-label-list" aria-label={`${name} labels`}>
                {labels.map((label, index) => (
                  <div className="signal-label-row" key={index}>
                    <ControlText
                      label={`Signal ${name} label ${index + 1}`}
                      value={label}
                      onCommit={(next) => onChange(changeSignalLabel(value, name, index, next))}
                    />
                    <button
                      className="icon-button control-remove"
                      title={`Remove label ${label}`}
                      aria-label={`Remove signal ${name} label ${label}`}
                      disabled={labels.length <= 1}
                      onClick={() =>
                        onChange({ ...value, [name]: labels.filter((_, i) => i !== index) })
                      }
                    >
                      <Trash2 size={13} />
                    </button>
                  </div>
                ))}
              </div>
              <button
                className="text-button control-add"
                onClick={() => onChange(appendSignalLabel(value, name))}
              >
                <Plus size={14} />
                Add label
              </button>
            </>
          ) : (
            <p className="helper">
              These labels use an unsupported format and are preserved unchanged.
            </p>
          )}
        </fieldset>
      ))}
      <button className="text-button control-add" onClick={() => onChange(addSignal(value))}>
        <Plus size={14} />
        Add signal
      </button>
      <p className="helper">
        Renaming or removing signals leaves existing conditions unchanged. Validate to check their
        references.
      </p>
    </section>
  );
}
