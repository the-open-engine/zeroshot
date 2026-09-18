import { Braces } from 'lucide-react';
import { allNodes, bindingFor, clone, executable, type Document } from './domain';
import { Field } from './Field';
import { ModelPicker } from './ModelPicker';

const harnessLabels = new Map([
  ['codex', 'Codex'],
  ['claude', 'Claude Code'],
  ['copilot', 'GitHub Copilot'],
]);

export function RuntimeEditor(p: {
  document: Document;
  schema: any;
  edit: (next: Document, key?: string) => void;
  openJson: () => void;
}) {
  const doc = p.document;
  const variants = p.schema?.oneOf ?? p.schema?.anyOf ?? [];
  const harnesses = variants
    .map((v: any) => v.properties?.harness?.const ?? v.properties?.harness?.enum?.[0])
    .filter(Boolean);
  const variant = variants.find(
    (v: any) =>
      (v.properties?.harness?.const ?? v.properties?.harness?.enum?.[0]) === doc.runtime.harness
  );
  const providerRef = variant?.properties?.provider?.$ref?.split('/').pop();
  const providers =
    p.schema?.$defs?.[providerRef]?.enum ?? variant?.properties?.provider?.enum ?? [];
  function runtimeChange(key: string, value: string) {
    const next = clone(doc);
    next.runtime[key] = value;
    if (key === 'harness') next.runtime.provider = '';
    p.edit(next, `runtime.${key}`);
  }
  return (
    <>
      <p className="runtime-scope">Applies to the whole graph.</p>
      <Field label="Harness">
        <select
          value={doc.runtime.harness}
          onChange={(e) => runtimeChange('harness', e.target.value)}
        >
          <option value="">Choose harness</option>
          {harnesses.map((h: string) => (
            <option key={h} value={h}>
              {harnessLabels.get(h) ?? h}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Provider">
        <select
          value={doc.runtime.provider}
          onChange={(e) => runtimeChange('provider', e.target.value)}
        >
          <option value="">Choose provider</option>
          {providers.map((v: string) => (
            <option key={v} value={v}>
              {v}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Run size">
        <select value={doc.runtime.size} onChange={(e) => runtimeChange('size', e.target.value)}>
          {(p.schema?.$defs?.RunSize?.enum ?? []).map((s: string) => (
            <option key={s}>{s}</option>
          ))}
        </select>
      </Field>
      <div className="section-rule" />
      <div className="section-title">
        Agent models{' '}
        <span className="muted">
          {
            allNodes(doc.graph.root).filter(
              (n) => bindingFor(doc.runtime, n.name)?.kind === 'agent'
            ).length
          }
        </span>
      </div>
      {allNodes(doc.graph.root)
        .filter(executable)
        .map((n) => (
          <div key={n.name} className="runtime-node">
            <span className="mono">{n.name}</span>
            {bindingFor(doc.runtime, n.name)?.kind === 'git_delivery' ? (
              <small>Git delivery</small>
            ) : bindingFor(doc.runtime, n.name)?.kind === 'agent' ? (
              <ModelPicker
                compact
                label={`Model for ${n.name}`}
                harness={doc.runtime.harness}
                provider={doc.runtime.provider}
                value={bindingFor(doc.runtime, n.name)?.model ?? ''}
                onChange={(value) => {
                  const next = clone(doc);
                  next.runtime.nodes = {
                    ...next.runtime.nodes,
                    [n.name]: {
                      ...bindingFor(next.runtime, n.name),
                      kind: 'agent',
                      model: value,
                    },
                  };
                  p.edit(next, `model.${n.name}`);
                }}
              />
            ) : (
              <small>Choose a worker in the node inspector.</small>
            )}
          </div>
        ))}
      <button className="text-button json-link" onClick={p.openJson}>
        <Braces size={15} /> Runtime JSON
      </button>
    </>
  );
}
