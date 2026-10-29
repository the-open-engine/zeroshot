import { Field } from './Field';
import { NumberField } from './NumberField';
import type { Binding } from './domain';

export function DecisionControls({
  node,
  binding,
  onChange,
}: {
  node: string;
  binding: Binding;
  onChange: (field: string, value: string | number) => void;
}) {
  return (
    <>
      <Field label="Decision model ID">
        <input
          value={binding.model ?? ''}
          placeholder="Model ID"
          onChange={(e) => onChange('model', e.target.value)}
        />
      </Field>
      <NumberField
        label="Minimum probability"
        node={node}
        field="minProbability"
        value={binding.minProbability}
        onChange={(value) => {
          if (value !== undefined) onChange('minProbability', value);
        }}
      />
      <Field
        label="Uncertain signal label"
        hint="Defined with the verifier’s signal and output schema in Node JSON."
      >
        <input value={binding.uncertainLabel ?? ''} readOnly />
      </Field>
      <small className="muted">
        Endpoint and API key come from the declared decision connection. Bind the evidence to this
        node’s inputs.
      </small>
    </>
  );
}
