import { useEffect, useId, useMemo, useState } from 'react';
import { ArrowRight, Link2 } from 'lucide-react';
import { executable, type Document, type GraphNode } from './domain';
import { bindingKey, pathLabel } from './bindings';
import { usePendingText } from './pending-edits';
import {
  connectData,
  allDataInputsMapped,
  connectionTypeLabel,
  dataInputFields,
  dataProducerChoices,
  existingDataConnections,
  previewDataConnection,
} from './data-connections';
import './data-connections.css';

export function DataConnections({
  document,
  node,
  onChange,
  selectNode,
}: {
  document: Document;
  node: GraphNode;
  onChange: (document: Document) => void;
  selectNode: (name: string) => void;
}) {
  const [targetId, setTargetId] = useState('');
  const [sourceId, setSourceId] = useState('');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  usePendingText(targetId !== '' || sourceId !== '');
  const id = useId();
  useEffect(() => {
    setTargetId('');
    setSourceId('');
    setError('');
    setNotice('');
  }, [node.name]);
  const targets = useMemo(() => dataInputFields(node), [node]);
  const sources = useMemo(() => dataProducerChoices(document, node), [document, node]);
  const existing = useMemo(() => existingDataConnections(document, node), [document, node]);
  const allMapped = allDataInputsMapped(node);
  const target = targets.find((field) => bindingKey(field.path) === targetId);
  const source = sources.find((choice) => choice.id === sourceId);
  const preview =
    target && source
      ? previewDataConnection(document, {
          consumer: node.name,
          target: target.path,
          source: source.source,
        })
      : undefined;
  if (!executable(node) && node.kind !== 'succeed') return null;
  const destination = node.kind === 'succeed' ? 'Output field' : 'Input field';
  return (
    <section className="data-connections" aria-label="Data connections">
      <h3>
        <Link2 size={15} aria-hidden="true" /> Data connections
      </h3>
      {existing.length > 0 && (
        <div className="data-existing">
          {existing.map((connection, index) => (
            <div className="data-connection" key={index}>
              <div className="data-connection-heading">
                <strong>{connection.target}</strong>
                <span>{connection.producers.length ? '' : connection.source}</span>
              </div>
              {connection.producers.map((producer) => (
                <button
                  type="button"
                  className="text-button"
                  key={producer.label}
                  onClick={() => selectNode(producer.name)}
                >
                  {producer.label} <ArrowRight size={12} aria-hidden="true" /> {connection.target}
                </button>
              ))}
              {!connection.producers.length && <p className="data-helper">{connection.note}</p>}
            </div>
          ))}
        </div>
      )}
      {!targets.length ? (
        <p className="data-helper">
          Add named fields to this node’s {node.kind === 'succeed' ? 'output' : 'input'} schema to
          connect producer data.
        </p>
      ) : (
        <details
          className="data-add-connection"
          key={`${node.name}-${allMapped ? 'mapped' : 'open'}`}
          open={!allMapped || targetId !== '' || sourceId !== ''}
        >
          <summary>Add connection</summary>
          <div className="data-connect-form">
            <label htmlFor={`${id}-target`}>{destination}</label>
            <select
              id={`${id}-target`}
              value={targetId}
              onChange={(event) => {
                setTargetId(event.target.value);
                setError('');
                setNotice('');
              }}
            >
              <option value="">Choose a field to supply</option>
              {targetId && !target && (
                <option value={targetId}>Selected field is no longer declared</option>
              )}
              {targets.map((field) => (
                <option key={bindingKey(field.path)} value={bindingKey(field.path)}>
                  {field.label}
                </option>
              ))}
            </select>
            <label htmlFor={`${id}-source`}>Producer field</label>
            <select
              id={`${id}-source`}
              value={sourceId}
              onChange={(event) => {
                setSourceId(event.target.value);
                setError('');
                setNotice('');
              }}
            >
              <option value="">Choose a node result</option>
              {sourceId && !source && (
                <option value={sourceId}>Selected producer field is no longer declared</option>
              )}
              {sources.map((choice) => (
                <option key={choice.id} value={choice.id}>
                  {choice.label}
                  {choice.available ? '' : ' · unavailable'}
                </option>
              ))}
            </select>
            {!sources.length && (
              <p className="data-helper">
                No named producer results are declared yet. Add an output, diagnostic, or signal
                field to an Agent or Verifier.
              </p>
            )}
            {source && <p className="data-helper">{source.reason}</p>}
            {preview && (
              <div className="data-route-preview" aria-label="Connection preview">
                <strong>
                  {source!.label} <ArrowRight size={13} aria-hidden="true" /> {node.name} ·{' '}
                  {pathLabel(target!.path)}
                </strong>
                {preview.canConnect ? (
                  <details className="data-route-details">
                    <summary>Connection details</summary>
                    {preview.reusesState ? (
                      <p>
                        Read the existing producer route through{' '}
                        <code>{pathLabel(preview.statePath)}</code> into {pathLabel(target!.path)}.
                        Its state fields, producer write, and group returns stay unchanged.
                      </p>
                    ) : (
                      <>
                        <p>
                          Add optional {connectionTypeLabel(preview)} field{' '}
                          <code>{pathLabel(preview.statePath)}</code> to{' '}
                          {preview.scopes.join(' → ')}.
                        </p>
                        <p>
                          Add a write from {source!.source.node}
                          {preview.promotions.length
                            ? ` and return it through ${preview.promotions.join(' → ')}`
                            : ''}
                          , then read it into {pathLabel(target!.path)}.
                        </p>
                      </>
                    )}
                    <p>
                      Applies as one edit. Existing fields, mappings, and runtime settings are
                      preserved.
                    </p>
                    {preview.notes.map((note) => (
                      <p className="data-helper" key={note}>
                        {note}
                      </p>
                    ))}
                  </details>
                ) : (
                  preview.reasons.map((reason) => (
                    <p className="data-unavailable" key={reason}>
                      {reason}
                    </p>
                  ))
                )}
              </div>
            )}
            <button
              type="button"
              className="button compact"
              disabled={!preview?.canConnect}
              onClick={() => {
                if (!preview) return;
                try {
                  onChange(connectData(document, preview.request));
                  setTargetId('');
                  setSourceId('');
                  setError('');
                  setNotice('Connected. Profile validation checks that the value is available.');
                } catch (cause) {
                  setError(
                    cause instanceof Error ? cause.message : 'The route could not be connected.'
                  );
                }
              }}
            >
              <Link2 size={14} aria-hidden="true" /> Connect
            </button>
          </div>
        </details>
      )}
      {notice && (
        <p className="data-helper" role="status">
          {notice}
        </p>
      )}
      {error && (
        <p className="data-unavailable" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
