import { useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { allNodes, type Document, type GraphNode } from './domain';
import { ControlText } from './ControlFields';
import { loopStopCandidates } from './loop-stop';
import {
  controlFields,
  controlLabels,
  controlSources,
  editableGuard,
  editableJoin,
  freshGuard,
  guardKinds,
  isObject,
  joinKinds,
  newGuardKind,
  positiveCount,
  wrapGuard,
  type ControlValue,
  type GuardValue,
} from './guards';
import './guards.css';

type Context = { document: Document; node?: GraphNode };
export type GuardEditorProps = Context & {
  value: GuardValue;
  onChange: (value: GuardValue) => void;
  label?: string;
  selectorNodes?: GraphNode[];
};

export function GuardEditor(props: GuardEditorProps) {
  return <GuardCondition {...props} label={props.label ?? 'Condition'} depth={0} />;
}

function GuardCondition({
  document,
  node,
  value,
  onChange,
  label,
  depth,
  selectorNodes,
}: GuardEditorProps & { label: string; depth: number }) {
  const [replacement, setReplacement] = useState('');
  const supported = editableGuard(value) && depth < 32;
  function changeKind(kind: string) {
    if (supported && kind === value.kind) return;
    const wrapped = isObject(value) && wrapGuard(value, kind);
    if (wrapped) onChange(wrapped);
    else setReplacement(kind);
  }
  const update = (patch: Record<string, any>) => onChange({ ...value, ...patch });
  return (
    <fieldset className="guard-condition">
      <legend>{label}</legend>
      <label className="control-label">
        Match
        <select
          aria-label={`${label} match`}
          value={supported ? value.kind : 'unsupported'}
          onChange={(event) => changeKind(event.target.value)}
        >
          {!supported && <option value="unsupported">Custom condition</option>}
          {Object.entries(guardKinds)
            .filter(([kind]) => ['in', 'all', 'any', 'not'].includes(kind) || kind === value?.kind)
            .map(([kind, title]) => (
              <option key={kind} value={kind} disabled={!!selectorNodes && kind === 'k_of_map'}>
                {title}
              </option>
            ))}
        </select>
      </label>
      {replacement && (
        <div className="control-confirm" role="alert">
          <p>Replace this condition and its current settings?</p>
          <div>
            <button className="button compact" onClick={() => setReplacement('')}>
              Keep condition
            </button>
            <button
              className="button compact"
              onClick={() => {
                onChange(newGuardKind(replacement, document, node, selectorNodes));
                setReplacement('');
              }}
            >
              Replace condition
            </button>
          </div>
        </div>
      )}
      {!supported ? null : value.kind === 'all' || value.kind === 'any' ? (
        <>
          {value.guards.map((guard: GuardValue, index: number) => (
            <div className="guard-child" key={index}>
              <GuardCondition
                document={document}
                node={node}
                value={guard}
                label={`${label} ${index + 1}`}
                depth={depth + 1}
                selectorNodes={selectorNodes}
                onChange={(next) =>
                  update({
                    guards: value.guards.map((item: GuardValue, i: number) =>
                      i === index ? next : item
                    ),
                  })
                }
              />
              <button
                className="text-button control-remove"
                disabled={value.guards.length <= 1}
                aria-label={`Remove ${label} ${index + 1}`}
                onClick={() =>
                  update({ guards: value.guards.filter((_: unknown, i: number) => i !== index) })
                }
              >
                <Trash2 size={13} />
                Remove condition
              </button>
            </div>
          ))}
          <button
            className="text-button control-add"
            onClick={() =>
              update({ guards: [...value.guards, freshGuard(document, node, selectorNodes)] })
            }
          >
            <Plus size={14} />
            Add condition
          </button>
        </>
      ) : value.kind === 'not' ? (
        <GuardCondition
          document={document}
          node={node}
          value={value.guard}
          label={`${label} negated`}
          depth={depth + 1}
          selectorNodes={selectorNodes}
          onChange={(guard) => update({ guard })}
        />
      ) : (
        <>
          {(value.kind === 'k_of_n' || value.kind === 'k_of_map') && (
            <div className="control-label">
              Minimum matches
              <ControlText
                label={`${label} minimum matches`}
                value={String(value.count ?? 1)}
                numeric
                onCommit={(count) => update({ count: positiveCount(count) })}
              />
            </div>
          )}
          {(value.kind === 'k_of_n' ? value.values : [value.value]).map(
            (selector: ControlValue, index: number) => (
              <div className="guard-selector" key={index}>
                <SelectorEditor
                  document={document}
                  choices={selectorNodes}
                  value={selector}
                  label={`${label} result ${index + 1}`}
                  onChange={(next) =>
                    value.kind === 'k_of_n'
                      ? update({
                          values: value.values.map((item: ControlValue, i: number) =>
                            i === index ? next : item
                          ),
                        })
                      : update({ value: next })
                  }
                />
                {value.kind === 'k_of_n' && (
                  <button
                    className="text-button control-remove"
                    disabled={value.values.length <= 1}
                    aria-label={`Remove ${label} result ${index + 1}`}
                    onClick={() =>
                      update({
                        values: value.values.filter((_: unknown, i: number) => i !== index),
                      })
                    }
                  >
                    <Trash2 size={13} />
                    Remove result
                  </button>
                )}
              </div>
            )
          )}
          {value.kind === 'k_of_n' && (
            <button
              className="text-button control-add"
              onClick={() =>
                update({
                  values: [...value.values, freshGuard(document, node, selectorNodes).value],
                })
              }
            >
              <Plus size={14} />
              Add result
            </button>
          )}
          <LabelChoices
            document={document}
            selectors={value.kind === 'k_of_n' ? value.values : [value.value]}
            value={value.labels}
            label={label}
            onChange={(labels) => update({ labels })}
          />
        </>
      )}
    </fieldset>
  );
}

function SelectorEditor({
  document,
  value,
  label,
  onChange,
  choices,
}: {
  document: Document;
  value: ControlValue;
  label: string;
  onChange: (value: ControlValue) => void;
  choices?: GraphNode[];
}) {
  const nodes =
    choices ?? allNodes(document.graph.root).filter((entry) => controlSources(entry).length);
  const options: { value: ControlValue; label: string }[] = nodes.flatMap((entry) =>
    entry.kind === 'verifier'
      ? Object.keys(controlFields(entry)).map((field) => ({
          value: { name: entry.name, source: 'signal', field },
          label: `${entry.name} · ${field}`,
        }))
      : []
  );
  const key = (entry: ControlValue) =>
    JSON.stringify([entry.name, entry.source, entry.field ?? null]);
  const selected = key(value);
  if (!options.some((option) => key(option.value) === selected))
    options.unshift({
      value,
      label: `${value.name || 'Choose result'}${value.field ? ` · ${value.field}` : value.source === 'error' ? ' · error' : ''}`,
    });
  return (
    <label className="control-label">
      Result
      <select
        aria-label={`${label} source`}
        value={selected}
        onChange={(event) => {
          const option = options.find((entry) => key(entry.value) === event.target.value);
          if (option) onChange({ ...value, ...option.value });
        }}
      >
        {options.map((option) => (
          <option key={key(option.value)} value={key(option.value)}>
            {option.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function LabelChoices({
  document,
  selectors,
  value,
  label,
  onChange,
}: {
  document: Document;
  selectors: ControlValue[];
  value: string[];
  label: string;
  onChange: (value: string[]) => void;
}) {
  const available = [
    ...new Set(selectors.flatMap((selector) => controlLabels(document, selector))),
  ];
  const options = [...new Set([...available, ...value])];
  return (
    <fieldset className="control-labels">
      <legend>Is one of</legend>
      {options.map((item) => (
        <label key={item}>
          <input
            type="checkbox"
            aria-label={`${label} label ${item}`}
            checked={value.includes(item)}
            disabled={value.includes(item) && value.length === 1}
            onChange={(event) =>
              onChange(
                event.target.checked ? [...value, item] : value.filter((entry) => entry !== item)
              )
            }
          />
          <span>
            {item}
            {!available.includes(item) && <small> (not declared here)</small>}
          </span>
        </label>
      ))}
    </fieldset>
  );
}

export function UntilEditor({
  value,
  onChange,
  label = 'Stop condition',
  ...context
}: Context & {
  value?: GuardValue | null;
  onChange: (value: GuardValue | undefined) => void;
  label?: string;
}) {
  const candidates = loopStopCandidates(context.node);
  return (
    <section className="control-section">
      <label className="control-check">
        <input
          type="checkbox"
          checked={value != null}
          disabled={value == null && !candidates.length}
          onChange={(event) =>
            onChange(
              event.target.checked
                ? freshGuard(context.document, context.node, candidates)
                : undefined
            )
          }
        />
        Stop when a condition matches
      </label>
      {value != null && (
        <GuardEditor
          {...context}
          value={value}
          onChange={onChange}
          label={label}
          selectorNodes={candidates}
        />
      )}
    </section>
  );
}

export function JoinEditor({
  value,
  onChange,
  label = 'Parallel join',
  ...context
}: GuardEditorProps) {
  const [replacement, setReplacement] = useState('');
  const known = editableJoin(value);
  const replace = (kind: string) => {
    onChange(
      kind === 'quorum'
        ? { kind, count: 1 }
        : kind === 'first'
          ? { kind, when: freshGuard(context.document, context.node) }
          : { kind }
    );
    setReplacement('');
  };
  return (
    <section className="control-section">
      <label className="control-label">
        {label}
        <select
          aria-label={label}
          value={known ? value.kind : 'unsupported'}
          onChange={(event) => {
            if (known && event.target.value === value.kind) return;
            if (
              !known ||
              !['all', 'any'].includes(value.kind) ||
              Object.keys(value).some((key) => key !== 'kind')
            )
              setReplacement(event.target.value);
            else replace(event.target.value);
          }}
        >
          {!known && <option value="unsupported">Unsupported join</option>}
          {Object.entries(joinKinds).map(([kind, title]) => (
            <option value={kind} key={kind}>
              {title}
            </option>
          ))}
        </select>
      </label>
      {replacement && (
        <div className="control-confirm" role="alert">
          <p>Replace the current join and its settings?</p>
          <div>
            <button className="button compact" onClick={() => setReplacement('')}>
              Keep join
            </button>
            <button className="button compact" onClick={() => replace(replacement)}>
              Replace join
            </button>
          </div>
        </div>
      )}
      {!known && <p className="helper">This join is preserved unchanged until you replace it.</p>}
      {known && value.kind === 'quorum' && (
        <div className="control-label">
          Required branches
          <ControlText
            label={`${label} count`}
            value={String(value.count ?? 1)}
            numeric
            onCommit={(count) => onChange({ ...value, count: positiveCount(count) })}
          />
        </div>
      )}
      {known && value.kind === 'first' && (
        <GuardEditor
          {...context}
          value={value.when}
          label={`${label} condition`}
          onChange={(when) => onChange({ ...value, when })}
        />
      )}
    </section>
  );
}
