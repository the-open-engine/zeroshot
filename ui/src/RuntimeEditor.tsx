import { isDeliveryKind } from './workers';
import { DecisionControls } from './DecisionControls';
import { useState } from 'react';
import { allNodes, bindingFor, clone, executable, type Document, type Runtime } from './domain';
import { Field } from './Field';
import { ModelPicker } from './ModelPicker';
import { modelGroups } from './models';
import { effectiveLane } from './runtime-readiness';
import type { HarnessOption } from './api';

export function RuntimeEditor(p: {
  document: Document;
  schema: any;
  harnesses?: readonly HarnessOption[];
  /** JSON path of the field the last validation rejected, e.g. `runtime.nodes.work.model`. */
  invalidField?: string;
  /** The runtime `invalidField` was found in; fields the draft changed since then are not flagged. */
  validated?: Runtime;
  /** Model IDs saved earlier with a harness and provider, offered as suggestions. */
  recentModels?: (harness: string, provider: string) => string[];
  edit: (next: Document, key?: string) => void;
}) {
  const doc = p.document;
  const nodes = allNodes(doc.graph.root).filter(executable);
  const kind = (name: string) => bindingFor(doc.runtime, name)?.kind;
  const agents = nodes.filter((n) => kind(n.name) === 'agent').map((n) => n.name);
  const model = (name: string) => bindingFor(doc.runtime, name)?.model ?? '';
  const shared = new Set(agents.map(model)).size <= 1;
  const lane = (name: string) => effectiveLane(doc.runtime, bindingFor(doc.runtime, name));
  const firstLane = lane(agents[0]);
  const mixedLanes = agents.some((name) => {
    const effective = lane(name);
    return effective.harness !== firstLane.harness || effective.provider !== firstLane.provider;
  });
  // A shared model field is meaningful only when all agents use the same harness and provider.
  const [showPerAgent, setPerAgent] = useState(!shared);
  const perAgent = mixedLanes || showPerAgent;
  const harnesses = p.harnesses ?? legacySchemaHarnesses(p.schema);
  const selectedHarness = harnesses.find((entry) => entry.id === doc.runtime.harness);
  const providers = selectedHarness?.providers ?? [];
  const unavailableHarness = !!doc.runtime.harness && !selectedHarness;
  const unavailableProvider = !!doc.runtime.provider && !providers.includes(doc.runtime.provider);
  const settingChanged = (key: string) => !!p.validated && doc.runtime[key] !== p.validated[key];
  const modelChanged = (name: string) =>
    !!p.validated && model(name) !== (bindingFor(p.validated, name)?.model ?? '');
  const invalid = (field: string, changed: boolean) =>
    (!changed && p.invalidField === `runtime.${field}`) || undefined;
  function runtimeChange(key: string, value: string) {
    const next = clone(doc);
    next.runtime[key] = value;
    if (key === 'harness') next.runtime.provider = '';
    p.edit(next, `runtime.${key}`);
  }
  function modelChange(names: string[], model: string) {
    const next = clone(doc);
    next.runtime.nodes = { ...next.runtime.nodes };
    for (const name of names)
      next.runtime.nodes[name] = { ...bindingFor(next.runtime, name), kind: 'agent', model };
    p.edit(next, names.length === 1 ? `model.${names[0]}` : 'model');
  }
  // One agent, or agents that agree, show their model; agents that differ show an empty field.
  const value = (names: string[]) => (shared || names.length === 1 ? model(names[0]) : '');
  const picker = (names: string[], label: string, extra: Partial<ModelPickerProps> = {}) => {
    const effective = lane(names[0]);
    const recent = p.recentModels?.(effective.harness, effective.provider) ?? [];
    return (
      <ModelPicker
        compact
        label={label}
        harness={effective.harness}
        provider={effective.provider}
        suggestions={modelGroups(doc.runtime, recent, value(names), effective)}
        value={value(names)}
        invalid={names.some((name) => invalid(`nodes.${name}.model`, modelChanged(name)))}
        onChange={(value) => modelChange(names, value)}
        {...extra}
      />
    );
  };
  // With one shared model, only nodes that still need a worker get their own row.
  const rows = nodes.filter(
    (n) => perAgent || !['agent', 'git_delivery', 'comment_delivery'].includes(kind(n.name) ?? '')
  );
  return (
    <>
      <p className="runtime-scope">Defaults for agents without a harness and provider override.</p>
      <Field label="Harness">
        <select
          value={doc.runtime.harness}
          aria-invalid={invalid('harness', settingChanged('harness'))}
          onChange={(e) => runtimeChange('harness', e.target.value)}
        >
          <option value="">Choose harness</option>
          {unavailableHarness && (
            <option value={doc.runtime.harness} disabled>
              {doc.runtime.harness} (unavailable)
            </option>
          )}
          {harnesses.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.label}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Provider">
        <select
          value={doc.runtime.provider}
          aria-invalid={invalid('provider', settingChanged('provider'))}
          onChange={(e) => runtimeChange('provider', e.target.value)}
        >
          <option value="">Choose provider</option>
          {unavailableProvider && (
            <option value={doc.runtime.provider} disabled>
              {doc.runtime.provider} (unavailable)
            </option>
          )}
          {providers.map((v: string) => (
            <option key={v} value={v}>
              {v}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Run size">
        <select
          value={doc.runtime.size}
          aria-invalid={invalid('size', settingChanged('size'))}
          onChange={(e) => runtimeChange('size', e.target.value)}
        >
          {(p.schema?.$defs?.RunSize?.enum ?? []).map((s: string) => (
            <option key={s}>{s}</option>
          ))}
        </select>
      </Field>
      <div className="section-rule" />
      <div className="section-title model-heading">
        <span>{perAgent ? 'Agent models' : 'Model'}</span>
        {agents.length > 1 && !mixedLanes && (
          <button
            type="button"
            className="text-button"
            aria-expanded={perAgent}
            onClick={() => setPerAgent(!perAgent)}
          >
            {perAgent ? 'Use one model for all' : 'Choose per agent'}
          </button>
        )}
      </div>
      {!perAgent && agents.length > 0 && (
        <div className="model-shared">
          {picker(agents, agents.length > 1 ? 'Model for all agents' : `Model for ${agents[0]}`, {
            placeholder: shared ? 'Model ID' : 'Different per agent',
          })}
          <small className="muted">Used by {listNames(agents)}.</small>
        </div>
      )}
      {rows.map((n) => (
        <div key={n.name} className="runtime-node">
          <span className="mono">{n.name}</span>
          {isDeliveryKind(kind(n.name)) ? (
            <small>
              {kind(n.name) === 'comment_delivery' ? 'Comment delivery' : 'Git delivery'}
            </small>
          ) : kind(n.name) === 'decision' ? (
            <DecisionControls
              node={n.name}
              binding={bindingFor(doc.runtime, n.name)!}
              onChange={(key, value) => {
                const next = clone(doc);
                if (value === '') delete next.runtime.nodes[n.name][key];
                else next.runtime.nodes[n.name][key] = value;
                p.edit(next, `binding.${n.name}.${key}`);
              }}
            />
          ) : kind(n.name) === 'agent' ? (
            picker([n.name], `Model for ${n.name}`)
          ) : (
            <small>Choose a worker in the node inspector.</small>
          )}
        </div>
      ))}
    </>
  );
}

type ModelPickerProps = Parameters<typeof ModelPicker>[0];

function listNames(names: string[]) {
  if (names.length === 1) return names[0];
  if (names.length > 4) return `${names.slice(0, 3).join(', ')} and ${names.length - 3} more`;
  return `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`;
}

// Version 1 hosts predating the explicit catalog advertised choices through enum variants.
function legacySchemaHarnesses(schema: any): HarnessOption[] {
  const variants = schema?.oneOf ?? schema?.anyOf ?? [];
  if (!Array.isArray(variants)) return [];
  return variants.flatMap((variant: any) => {
    const properties = variant?.properties;
    const id = properties?.harness?.const ?? properties?.harness?.enum?.[0];
    if (typeof id !== 'string' || !id) return [];
    const provider = properties?.provider;
    const reference = typeof provider?.$ref === 'string' ? provider.$ref.split('/').pop() : '';
    const values = schema?.$defs?.[reference]?.enum ?? provider?.enum ?? [];
    const providers = Array.isArray(values)
      ? values.filter((value): value is string => typeof value === 'string' && !!value)
      : [];
    return [{ id, label: id, providers }];
  });
}
