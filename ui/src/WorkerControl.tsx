import { useEffect, useState } from 'react';
import { bindingFor, type Document, type GraphNode } from './domain';
import { Field } from './Field';
import { applyWorker, scriptCommandLabel, workerOption, type WorkerOption } from './workers';

export function WorkerControl({
  document,
  node,
  workers,
  onChange,
}: {
  document: Document;
  node: GraphNode;
  workers: WorkerOption[];
  onChange: (document: Document) => void;
}) {
  const binding = bindingFor(document.runtime, node.name);
  const current = workerOption(node, binding, workers);
  const script = binding?.kind === 'script';
  const selected = current?.id ?? (script ? 'script' : 'unsupported');
  const [pending, setPending] = useState('');
  const [error, setError] = useState('');
  useEffect(() => {
    setPending('');
    setError('');
  }, [node.name]);
  const replacement = workers.find((option) => option.id === pending);
  return (
    <section className="control-section" aria-label="Worker implementation">
      <Field label="Worker">
        <select
          value={selected}
          onChange={(event) => {
            setError('');
            if (event.target.value !== current?.id || binding?.kind !== current?.runtimeKind)
              setPending(event.target.value);
          }}
        >
          {!current && (
            <option value={selected}>{script ? 'Script' : 'Unrecognized worker binding'}</option>
          )}
          {workers.map((option) => (
            <option key={option.id} value={option.id}>
              {option.label}
            </option>
          ))}
        </select>
      </Field>
      {script && <code aria-label="Script command">{scriptCommandLabel(binding.command)}</code>}
      {replacement && (
        <div className="control-confirm" role="alert">
          <p>Change this node to {replacement.label}?</p>
          <p className="helper">
            {replacement.runtimeKind === 'agent'
              ? 'This replaces the worker implementation with an agent.'
              : 'This resets the node’s inputs, outputs, instructions, and runtime settings.'}
          </p>
          <div>
            <button className="button compact" onClick={() => setPending('')}>
              Keep worker
            </button>
            <button
              className="button compact"
              onClick={() => {
                try {
                  onChange(applyWorker(document, node.name, replacement, workers));
                  setPending('');
                  setError('');
                } catch (error) {
                  setError((error as Error).message);
                }
              }}
            >
              Replace worker
            </button>
          </div>
        </div>
      )}
      {error && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
