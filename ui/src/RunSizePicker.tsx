import { Field } from './Field';
import type { MachineOption } from './machines';
import type { RunSizeCatalog } from './run-sizes';

export function RunSizePicker(p: {
  value?: string;
  runSizes?: RunSizeCatalog;
  machines?: readonly MachineOption[];
  invalid?: boolean;
  onChange: (value: string | undefined) => void;
}) {
  const offered = p.runSizes?.sizes.map(({ id }) => id) ?? p.machines?.map(({ size }) => size);
  const machines = p.machines?.filter((machine) => offered?.includes(machine.size));
  const choices = offered?.map((id) => {
    const machine = machines?.find(({ size }) => size === id);
    return {
      value: id,
      label: machine
        ? `${machine.label} · ${machine.laneUnits} ${machine.laneUnits === 1 ? 'lane' : 'lanes'}`
        : id,
    };
  });
  const missing = !!p.value && !choices?.some((option) => option.value === p.value);
  const unavailable = missing && offered !== undefined;
  const selected = machines?.find((machine) => machine.size === (p.value ?? p.runSizes?.default));
  const defaultLabel = p.runSizes?.default
    ? `Target default (${p.runSizes.default})`
    : 'Target default';
  return (
    <Field label="Run size">
      {choices === undefined ? (
        <input
          value={p.value ?? ''}
          placeholder={defaultLabel}
          aria-invalid={p.invalid}
          onChange={(event) => p.onChange(event.target.value || undefined)}
        />
      ) : (
        <select
          value={p.value ?? ''}
          aria-invalid={p.invalid}
          onChange={(event) => p.onChange(event.target.value || undefined)}
        >
          <option value="" disabled={choices.length === 0}>
            {choices.length === 0 ? 'No sizes available' : defaultLabel}
          </option>
          {missing && (
            <option value={p.value} disabled={unavailable}>
              {p.value}
              {unavailable ? ' (unavailable)' : ''}
            </option>
          )}
          {choices.map(({ value, label }) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
      )}
      {selected && (
        <small>
          Starting this size with Boost uses {selected.laneUnits}{' '}
          {selected.laneUnits === 1 ? 'Boost' : 'Boosts'} for the run.
        </small>
      )}
    </Field>
  );
}
