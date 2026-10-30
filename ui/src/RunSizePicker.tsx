import { Field } from './Field';
import type { MachineOption } from './machines';

export function RunSizePicker(p: {
  value: string;
  schema: any;
  machines?: readonly MachineOption[];
  invalid?: boolean;
  onChange: (value: string) => void;
}) {
  const nativeSizes: string[] = p.schema?.$defs?.RunSize?.enum ?? [];
  const machines = p.machines?.filter((machine) => nativeSizes.includes(machine.size));
  const choices =
    machines?.map((machine) => ({
      value: machine.size,
      label: `${machine.label} · ${machine.laneUnits} ${machine.laneUnits === 1 ? 'lane' : 'lanes'}`,
    })) ?? nativeSizes.map((value) => ({ value, label: value }));
  const missing = !!p.value && !choices.some((option) => option.value === p.value);
  const unavailable = missing && (p.machines !== undefined || nativeSizes.length > 0);
  const selected = machines?.find((machine) => machine.size === p.value);
  return (
    <Field label="Run size">
      <select
        value={p.value}
        aria-invalid={p.invalid}
        onChange={(event) => p.onChange(event.target.value)}
      >
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
      {selected && (
        <small>
          Starting this size with Boost uses {selected.laneUnits}{' '}
          {selected.laneUnits === 1 ? 'Boost' : 'Boosts'} for the run.
        </small>
      )}
    </Field>
  );
}
