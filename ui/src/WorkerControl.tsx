import { useEffect, useState } from 'react';
import { bindingFor, type Document, type GraphNode } from './domain';
import { Field } from './Field';
import { applyWorker, workerOption, type WorkerOption } from './workers';

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
          value={current?.id ?? 'unsupported'}
          onChange={(event) => {
            setError('');
            if (event.target.value !== current?.id || binding?.kind !== current?.runtimeKind)
              setPending(event.target.value);
          }}
        >
          {!current && <option value="unsupported">Unrecognized worker binding</option>}
          {workers.map((option) => (
            <option key={option.id} value={option.id}>
              {option.label}
            </option>
          ))}
        </select>
      </Field>
      {replacement && (
        <div className="control-confirm" role="alert">
          <p>Change this node to {replacement.label}?</p>
          <p className="helper">
            {replacement.runtimeKind === 'git_delivery'
              ? 'This resets the node’s inputs, outputs, instructions, and runtime settings.'
              : 'This replaces the delivery implementation with an agent.'}
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
