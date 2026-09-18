import { useId, useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { executable, isGroup, type Document, type GraphNode } from './domain';
import { usePendingText } from './pending-edits';
import {
  appendMapping,
  bindingContext,
  bindingKey,
  dataChoices,
  mapOverChoices,
  outputChoices,
  pathChoices,
  pathLabel,
  removeMapping,
  selectorLabel,
  togglePromotion,
  updateMapping,
  type BindingChoice,
} from './bindings';
import { connectPromotionSource, promotionTraces, type PromotionTrace } from './promotion-sources';
import './bindings.css';

export type BindingsEditorProps = {
  document: Document;
  node: GraphNode;
  onChange: (next: GraphNode) => void;
  selectNode?: (name: string) => void;
};

export function BindingsEditor({ document, node, onChange, selectNode }: BindingsEditorProps) {
  const context = bindingContext(document, node);
  const data = dataChoices(context);
  return (
    <div className="bindings-editor">
      {executable(node) && (
        <>
          <MappingSection
            key={`${node.name}-inputs`}
            title="Input mappings"
            field="inputBindings"
            node={node}
            onChange={onChange}
            targets={pathChoices(node.input)}
            sources={data}
            destinationLabel="Input field"
            description={`Read fields from ${context.stateName}${context.itemName ? ` or an item of ${context.itemName}` : ''}.`}
          />
          <MappingSection
            key={`${node.name}-writes`}
            title="State writes"
            field="writeBindings"
            node={node}
            onChange={onChange}
            targets={pathChoices(context.state)}
            sources={outputChoices(node)}
            destinationLabel="State field"
            description={`Write this node's results into ${context.stateName}.`}
          />
        </>
      )}
      {node.kind === 'succeed' && (
        <MappingSection
          key={`${node.name}-result`}
          title="Output mappings"
          field="bindings"
          node={node}
          onChange={onChange}
          targets={pathChoices(node.output)}
          sources={data}
          destinationLabel="Output field"
          description="Choose the state or item fields returned by this success node."
        />
      )}
      {node.kind === 'map' && (
        <section className="bindings-section" aria-label="Map items">
          <h3>Map items</h3>
          <p className="bindings-helper">Run the body once for each item in this array.</p>
          <ChoiceSelect
            label="Array to map over"
            choices={mapOverChoices(node, context)}
            value={node.over}
            describe={selectorLabel}
            onChange={(over) => onChange({ ...node, over })}
          />
        </section>
      )}
      {isGroup(node) && (
        <Promotions
          key={`${node.name}-promotions`}
          document={document}
          node={node}
          onChange={onChange}
          selectNode={selectNode}
        />
      )}
    </div>
  );
}

function ChoiceSelect({
  label,
  choices,
  value,
  onChange,
  describe,
}: {
  label: string;
  choices: BindingChoice[];
  value: any;
  onChange: (value: any) => void;
  describe: (value: any) => string;
}) {
  const id = useId();
  const key = value === undefined ? '' : bindingKey(value);
  const unresolved =
    value !== undefined && !choices.some((choice) => bindingKey(choice.value) === key);
  return (
    <div className="bindings-field">
      <label htmlFor={id}>{label}</label>
      <select
        id={id}
        value={key}
        onChange={(event) => {
          const choice = choices.find(
            (candidate) => bindingKey(candidate.value) === event.target.value
          );
          if (choice) onChange(choice.value);
        }}
      >
        <option value="" disabled>
          Choose a field
        </option>
        {unresolved && <option value={key}>{describe(value)} · unresolved</option>}
        {choices.map((choice) => (
          <option key={bindingKey(choice.value)} value={bindingKey(choice.value)}>
            {choice.label}
          </option>
        ))}
      </select>
      {unresolved && (
        <small className="bindings-unresolved">
          Existing selection preserved. Its field is not available here.
        </small>
      )}
    </div>
  );
}

function MappingSection({
  title,
  field,
  node,
  onChange,
  targets,
  sources,
  destinationLabel,
  description,
}: {
  title: string;
  field: string;
  node: GraphNode;
  onChange: (next: GraphNode) => void;
  targets: BindingChoice[];
  sources: BindingChoice[];
  destinationLabel: string;
  description: string;
}) {
  const [adding, setAdding] = useState(false);
  const [target, setTarget] = useState<any>();
  const [value, setValue] = useState<any>();
  usePendingText(adding && (target !== undefined || value !== undefined));
  const unsupported = node[field] !== undefined && !Array.isArray(node[field]);
  const mappings: any[] = Array.isArray(node[field]) ? node[field] : [];
  const unusedTargets = targets.filter(
    (choice) =>
      !mappings.some((mapping) => bindingKey(mapping?.target) === bindingKey(choice.value))
  );
  const canAdd = !unsupported && unusedTargets.length > 0 && sources.length > 0;
  const draftReady =
    unusedTargets.some((choice) => bindingKey(choice.value) === bindingKey(target)) &&
    sources.some((choice) => bindingKey(choice.value) === bindingKey(value));
  function cancelDraft() {
    setAdding(false);
    setTarget(undefined);
    setValue(undefined);
  }
  // Null payloads have no bindable fields. Existing authored mappings remain visible.
  if (!unsupported && !adding && mappings.length === 0 && (!targets.length || !sources.length))
    return null;
  return (
    <section className="bindings-section" aria-label={title}>
      <div className="bindings-heading">
        <h3>{title}</h3>
        {mappings.length > 0 && <span>{mappings.length}</span>}
      </div>
      <p className="bindings-helper">{description}</p>
      {unsupported ? (
        <p className="bindings-unresolved">
          Existing mappings use an unsupported format and are preserved.
        </p>
      ) : null}
      {mappings.map((mapping, index) => (
        <fieldset className="bindings-row" key={index}>
          <legend>
            {title} {index + 1}
          </legend>
          <ChoiceSelect
            label={`${destinationLabel} · mapping ${index + 1}`}
            choices={targets}
            value={mapping?.target}
            describe={pathLabel}
            onChange={(next) => onChange(updateMapping(node, field, index, { target: next }))}
          />
          <ChoiceSelect
            label={`Source · ${title.toLowerCase()} ${index + 1}`}
            choices={sources}
            value={mapping?.value}
            describe={selectorLabel}
            onChange={(next) => onChange(updateMapping(node, field, index, { value: next }))}
          />
          <button
            type="button"
            className="bindings-remove"
            aria-label={`Remove ${title.toLowerCase()} mapping ${index + 1}`}
            onClick={() => onChange(removeMapping(node, field, index))}
          >
            <Trash2 size={13} aria-hidden="true" /> Remove
          </button>
        </fieldset>
      ))}
      {adding ? (
        <fieldset className="bindings-row bindings-draft">
          <legend>New {title.toLowerCase()} mapping</legend>
          <ChoiceSelect
            label={`${destinationLabel} · new mapping`}
            choices={unusedTargets}
            value={target}
            describe={pathLabel}
            onChange={setTarget}
          />
          <ChoiceSelect
            label={`Source · new ${title.toLowerCase()} mapping`}
            choices={sources}
            value={value}
            describe={selectorLabel}
            onChange={setValue}
          />
          <div className="bindings-actions">
            <button
              type="button"
              className="button compact"
              disabled={!draftReady}
              aria-label={`Add ${title.toLowerCase()} mapping`}
              onClick={() => {
                if (!draftReady) return;
                onChange(appendMapping(node, field, target, value));
                cancelDraft();
              }}
            >
              Add mapping
            </button>
            <button type="button" className="text-button" onClick={cancelDraft}>
              Cancel
            </button>
          </div>
        </fieldset>
      ) : (
        <button
          type="button"
          className="text-button bindings-add"
          disabled={!canAdd}
          aria-label={`Add ${title.toLowerCase()} mapping`}
          onClick={() => setAdding(true)}
        >
          <Plus size={14} aria-hidden="true" /> Add mapping
        </button>
      )}
      {!canAdd && !adding && !unsupported && (
        <p className="bindings-helper">
          {targets.length === 0
            ? `Add an object field to the ${destinationLabel.toLowerCase().replace(' field', '')} schema to create a mapping.`
            : sources.length === 0
              ? 'No source fields are available. Add fields to the source schema first.'
              : 'All destination fields are already mapped.'}
        </p>
      )}
    </section>
  );
}

function Promotions({ document, node, onChange, selectNode }: BindingsEditorProps) {
  const [adding, setAdding] = useState(false);
  const [draftKey, setDraftKey] = useState('');
  const chooserId = useId();
  usePendingText(adding && draftKey !== '');
  if (document.graph.root.name === node.name) {
    const metadata = node.promotedStatePaths;
    if (metadata === undefined || (Array.isArray(metadata) && metadata.length === 0)) return null;
    return (
      <section className="bindings-section" aria-label="Root promotion metadata">
        <details className="promotion-card">
          <summary>Root promotion metadata</summary>
          <div className="promotion-content">
            <p className="bindings-helper">
              The root has no parent. Configure the run result on a Success node. Existing promotion
              metadata is preserved and can be inspected or edited in Node JSON.
            </p>
            {Array.isArray(metadata) && (
              <p className="promotion-chain">{metadata.map(pathLabel).join(', ')}</p>
            )}
          </div>
        </details>
      </section>
    );
  }
  const traces = promotionTraces(document, node);
  const authored = traces.filter((trace) => trace.selected);
  const available = traces.filter((trace) => !trace.selected);
  const preview = available.find((trace) => bindingKey(trace.path) === draftKey);
  const count = Array.isArray(node.promotedStatePaths) ? node.promotedStatePaths.length : 0;
  const unsupported =
    node.promotedStatePaths !== undefined && !Array.isArray(node.promotedStatePaths);
  function closeChooser() {
    setAdding(false);
    setDraftKey('');
  }
  return (
    <section className="bindings-section" aria-label="Promoted state fields">
      <div className="bindings-heading">
        <h3>Promoted state fields</h3>
        <span aria-label={`${count} promoted state fields`}>{count}</span>
      </div>
      {unsupported && (
        <p className="bindings-unresolved">
          Existing promotions use an unsupported format and are preserved.
        </p>
      )}
      {authored.map((trace, index) => (
        <PromotionCard
          key={`${bindingKey(trace.path)}-${index}`}
          document={document}
          node={node}
          trace={trace}
          onChange={onChange}
          selectNode={selectNode}
          disabled={unsupported}
        />
      ))}
      {adding ? (
        <div className="promotion-add">
          <label htmlFor={chooserId}>State field to promote</label>
          <select
            id={chooserId}
            value={draftKey}
            onChange={(event) => setDraftKey(event.target.value)}
          >
            <option value="">Choose a field</option>
            {available.map((trace) => (
              <option key={bindingKey(trace.path)} value={bindingKey(trace.path)}>
                {trace.label}
              </option>
            ))}
          </select>
          {preview && (
            <PromotionCard
              key={bindingKey(preview.path)}
              document={document}
              node={node}
              trace={preview}
              preview
              onChange={(next) => {
                onChange(next);
                closeChooser();
              }}
              selectNode={selectNode}
              disabled={unsupported}
            />
          )}
          <button
            type="button"
            className="text-button"
            aria-label="Cancel adding promoted field"
            onClick={closeChooser}
          >
            Cancel
          </button>
        </div>
      ) : (
        !unsupported && (
          <button
            type="button"
            className="text-button bindings-add"
            disabled={!available.length}
            onClick={() => setAdding(true)}
          >
            <Plus size={14} aria-hidden="true" /> Add promoted field
          </button>
        )
      )}
      {!traces.length && !unsupported && (
        <p className="bindings-helper">Add a state field first.</p>
      )}
    </section>
  );
}

function PromotionCard({
  document,
  node,
  onChange,
  selectNode,
  trace,
  disabled,
  preview = false,
}: BindingsEditorProps & { trace: PromotionTrace; disabled: boolean; preview?: boolean }) {
  const [chosen, setChosen] = useState('');
  const [error, setError] = useState('');
  const id = useId();
  usePendingText(chosen !== '');
  const existing = trace.sources.filter((source) => source.existing);
  const ready = existing.some((source) => source.connected);
  const candidates = trace.sources.filter((source) => !source.existing && source.canConnect);
  const unavailable = trace.sources.filter((source) => !source.existing && !source.canConnect);
  const status = !trace.selected
    ? existing.some((source) => source.canConnect)
      ? 'Child source available'
      : existing.length || trace.issues.length
        ? 'Not promoted'
        : 'No child producer'
    : trace.issues.length
      ? 'Needs a state field'
      : ready
        ? 'Mapping route connected'
        : existing.length
          ? 'Child route needs attention'
          : trace.passThrough
            ? 'Incoming value only'
            : 'No child producer';
  function connect(sourceId: string) {
    try {
      onChange(connectPromotionSource(document, node, trace.path, sourceId));
      setChosen('');
      setError('');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'This source could not be connected.');
    }
  }
  return (
    <details className="promotion-card" open={trace.selected || preview}>
      <summary>
        <strong>{trace.label}</strong>
        <span>
          {trace.selected ? 'Promoted · ' : ''}
          {status}
        </span>
        {trace.schemaNote && <span>{trace.schemaNote}</span>}
      </summary>
      <div className="promotion-content">
        <p className="promotion-destination">
          {node.name} · {trace.label} ({trace.fieldType}) → {trace.parent} · {trace.label} (
          {trace.parentType})
        </p>
        {trace.issues.map((issue) => (
          <p className="bindings-unresolved" key={issue}>
            {issue}
          </p>
        ))}
        {existing.map((source) => (
          <div className="promotion-route" key={source.id}>
            <p className="promotion-chain">
              {source.label} → {source.chain.join(' → ')} → {trace.parent}
            </p>
            {source.missing.length > 0 && (
              <p className="bindings-helper">Missing promotion: {source.missing.join(', ')}.</p>
            )}
            {source.issues.map((issue) => (
              <p className="bindings-unresolved" key={issue}>
                {issue}
              </p>
            ))}
            {source.notes.length > 0 && (
              <details className="promotion-unavailable">
                <summary>Completion conditions</summary>
                {source.notes.map((note) => (
                  <p className="bindings-helper" key={note}>
                    {note}
                  </p>
                ))}
              </details>
            )}
            <div className="bindings-actions">
              {source.canConnect && source.missing.length > 0 && (
                <button
                  type="button"
                  className="text-button"
                  disabled={disabled}
                  onClick={() => connect(source.id)}
                  aria-label={`Connect ${trace.label} through wrappers from ${source.writer}`}
                >
                  Connect through wrappers
                </button>
              )}
              {selectNode && (
                <button
                  type="button"
                  className="text-button"
                  onClick={() => selectNode(source.writer)}
                  aria-label={`Show producer ${source.writer} for ${trace.label}`}
                >
                  Show {source.writer}
                </button>
              )}
            </div>
          </div>
        ))}
        {!existing.length && (
          <p className="bindings-helper">
            No child currently writes this field. Select a child output to create the route.
          </p>
        )}
        {!ready && candidates.length > 0 && !disabled && (
          <div className="promotion-connect">
            <label htmlFor={id}>Child source for {trace.label}</label>
            <select
              id={id}
              value={chosen}
              onChange={(event) => {
                setChosen(event.target.value);
                setError('');
              }}
            >
              <option value="">Select a child output</option>
              {candidates.map((source) => (
                <option key={source.id} value={source.id}>
                  {source.label}
                </option>
              ))}
            </select>
            {chosen && (
              <p className="bindings-helper">
                Adds this child’s state write and the required promotions through its wrappers.
                Validate checks that the result is available on the required paths.
              </p>
            )}
            <button
              type="button"
              className="button compact"
              disabled={!candidates.some((source) => source.id === chosen)}
              onClick={() => connect(chosen)}
              aria-label={`Connect ${trace.label} from child`}
            >
              Connect source
            </button>
          </div>
        )}
        {!ready && !candidates.length && !existing.length && (
          <p className="bindings-helper">
            No child output has a confirmed matching route. Add an output or diagnostic field to a
            child and matching state fields along its wrappers.
          </p>
        )}
        {!ready && unavailable.length > 0 && (
          <details className="promotion-unavailable">
            <summary>Why other sources cannot connect</summary>
            {unavailable.map((source) => (
              <p className="bindings-helper" key={source.id}>
                <strong>{source.label}</strong>
                <br />
                {source.issues[0]}
              </p>
            ))}
          </details>
        )}
        {!ready && <p className="bindings-helper">{trace.passThroughNote}</p>}
        {trace.passThrough && !trace.selected && !ready && (
          <button
            type="button"
            className="text-button"
            disabled={disabled}
            onClick={() => onChange(togglePromotion(node, trace.path, true))}
            aria-label={`Pass through incoming ${trace.label}`}
          >
            Keep incoming value
          </button>
        )}
        {trace.selected && (
          <button
            type="button"
            className="bindings-remove"
            disabled={disabled}
            onClick={() => onChange(togglePromotion(node, trace.path, false))}
            aria-label={`Remove promotion ${trace.label}`}
          >
            <Trash2 size={13} aria-hidden="true" /> Remove promotion
          </button>
        )}
        {error && (
          <p className="bindings-unresolved" role="alert">
            {error}
          </p>
        )}
      </div>
    </details>
  );
}
