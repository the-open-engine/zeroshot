import { useEffect, useRef, useState } from 'react';
import { Field } from './Field';
import { NumberField } from './NumberField';
import { useNumericDrafts } from './numeric-drafts';
export { Field } from './Field';
import { ArrowDown, ArrowUp, Braces, ChevronRight, Trash2, X } from 'lucide-react';
import {
  bindingFor,
  children,
  clone,
  executable,
  labels,
  replaceNode,
  type Document,
  type GraphNode,
} from './domain';
import { WorkerControl } from './WorkerControl';
import type { WorkerOption } from './workers';
import { GuardEditor } from './GuardEditor';
import { reviewSources } from './simple-controls';
import { ModelPicker } from './ModelPicker';
import { usePendingText } from './pending-edits';
import { NodeDataEditor } from './NodeDataEditor';
import { OutcomeSettings } from './OutcomeSettings';
import { analyzeWorkflowOutcomes } from './workflow-outcomes';
import { editableActivityRole, makeReadOnlyVerifier, makeWritingAgent } from './activity-access';
export type InspectorProps = {
  document: Document;
  node?: GraphNode;
  parent?: GraphNode;
  edit: (d: Document, key?: string) => void;
  updateNode: (n: GraphNode) => void;
  openJson: (target: string) => void;
  openRuntime: () => void;
  move: (offset: number) => void;
  remove: () => void;
  close: () => void;
  schema: any;
  workers: WorkerOption[];
  replaceBody: (name: string) => void;
  wrapBody: (name: string) => void;
  selectNode: (name: string) => void;
  addOtherwise: (name: string) => void;
};
export function Inspector(p: InspectorProps) {
  const numericDrafts = useNumericDrafts();
  const { document: doc, node } = p;
  const scroll = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (scroll.current) scroll.current.scrollTop = 0;
  }, [node?.name]);
  const binding = node ? bindingFor(doc.runtime, node.name) : undefined;
  const delivery = binding?.kind === 'git_delivery';
  const foldedFailures = new Set(
    analyzeWorkflowOutcomes(doc)
      .policies.filter((policy) => policy.choiceName === node?.name)
      .map((policy) => policy.branchIndex)
  );
  const graphChange = (key: string, value: any) => {
    if (node) p.updateNode({ ...node, [key]: value });
  };
  return (
    <aside className="inspector" aria-label="Profile inspector">
      <div className="inspector-tabs">
        <span className="inspector-label">Node</span>
        <button
          className="icon-button close-inspector"
          title="Close inspector"
          aria-label="Close inspector"
          onClick={p.close}
        >
          <X size={17} />
        </button>
      </div>
      <div className="inspector-scroll" ref={scroll}>
        {node ? (
          <>
            <div className="inspector-heading">
              <h2>
                {delivery
                  ? 'Git delivery'
                  : node.name === doc.graph.root.name
                    ? 'Run settings'
                    : node.kind === 'choice'
                      ? 'Decision'
                      : node.kind === 'succeed'
                        ? 'Run result'
                        : node.kind === 'fail'
                          ? 'Stop outcome'
                          : labels[node.kind]}
              </h2>
            </div>
            <NameField
              key={node.name}
              name={node.name}
              onChange={(value) => {
                p.updateNode({ ...node, name: value });
                numericDrafts.rename(node.name, value);
              }}
            />
            {p.parent?.kind === 'choice' &&
              p.parent.branches.some((b: any) => b.node.name === node.name) && (
                <details className="schema-editor" open>
                  <summary>Branch condition</summary>
                  <GuardEditor
                    key={node.name}
                    document={doc}
                    node={p.parent}
                    value={p.parent.branches.find((b: any) => b.node.name === node.name).when}
                    onChange={(when) => {
                      const parent = p.parent!;
                      p.edit(
                        replaceNode(doc, parent.name, {
                          ...parent,
                          branches: parent.branches.map((b: any) =>
                            b.node.name === node.name ? { ...b, when } : b
                          ),
                        })
                      );
                    }}
                  />
                </details>
              )}
            {p.parent && (
              <button
                className="group-open"
                aria-label={`Containing group: ${p.parent.name}`}
                onClick={() => p.selectNode(p.parent!.name)}
              >
                In{' '}
                <span>
                  {p.parent.name}
                  <ChevronRight size={16} />
                </span>
              </button>
            )}
            {node.kind === 'choice' && (
              <div className="choice-branches">
                {node.branches.map((branch: any, i: number) =>
                  foldedFailures.has(i) ? null : (
                    <button
                      key={branch.node.name}
                      className="group-open"
                      onClick={() => p.selectNode(branch.node.name)}
                    >
                      Branch {i + 1}
                      <span>
                        {branch.node.name}
                        <ChevronRight size={14} />
                      </span>
                    </button>
                  )
                )}
                {node.otherwise ? (
                  <button className="group-open" onClick={() => p.selectNode(node.otherwise.name)}>
                    Otherwise
                    <span>
                      {node.otherwise.name}
                      <ChevronRight size={14} />
                    </span>
                  </button>
                ) : (
                  <button className="text-button" onClick={() => p.addOtherwise(node.name)}>
                    Add otherwise branch
                  </button>
                )}
              </div>
            )}
            <ActivitySettings
              p={p}
              node={node}
              binding={binding}
              delivery={delivery}
              graphChange={graphChange}
            />
            {node.kind === 'loop' && (
              <div className="loop-reviewers">
                {reviewSources(node).map((name) => (
                  <button key={name} className="group-open" onClick={() => p.selectNode(name)}>
                    Review{' '}
                    <span>
                      {name.replaceAll('_', ' ')}
                      <ChevronRight size={14} />
                    </span>
                  </button>
                ))}
              </div>
            )}
            {node.kind === 'loop' && (
              <NumberField
                key={`${node.name}-rounds`}
                node={node.name}
                field="maxIterations"
                label={reviewSources(node).length ? 'Maximum attempts' : 'Rounds'}
                value={node.maxIterations}
                onChange={(value) => graphChange('maxIterations', value)}
              />
            )}
            {node.kind === 'map' && (
              <NumberField
                key={`${node.name}-items`}
                node={node.name}
                field="maxItems"
                label="Maximum items"
                value={node.maxItems}
                onChange={(value) => graphChange('maxItems', value)}
              />
            )}
            {node.kind === 'fail' && (
              <Field label="Failure reason">
                <input
                  value={node.reason ?? ''}
                  onChange={(e) => graphChange('reason', e.target.value)}
                />
              </Field>
            )}
            <OutcomeSettings
              key={`${node.name}-outcomes`}
              document={doc}
              node={node}
              edit={p.edit}
              selectNode={p.selectNode}
            />
            <NodeDataEditor
              key={`${node.name}-data`}
              document={doc}
              node={node}
              edit={p.edit}
              openJson={p.openJson}
              selectNode={p.selectNode}
            />
            {p.parent && ['seq', 'par', 'choice'].includes(p.parent.kind) && (
              <div className="order-control">
                <span>{p.parent.kind === 'seq' ? 'Execution order' : 'Branch order'}</span>
                <div>
                  <button
                    className="icon-button"
                    title="Move earlier"
                    aria-label="Move earlier"
                    disabled={
                      children(p.parent)[0]?.name === node.name ||
                      (p.parent.kind === 'choice' && p.parent.otherwise?.name === node.name)
                    }
                    onClick={() => p.move(-1)}
                  >
                    <ArrowUp size={16} />
                  </button>
                  <button
                    className="icon-button"
                    title="Move later"
                    aria-label="Move later"
                    disabled={
                      children(p.parent).at(-1)?.name === node.name ||
                      (p.parent.kind === 'choice' &&
                        p.parent.branches.at(-1)?.node.name === node.name)
                    }
                    onClick={() => p.move(1)}
                  >
                    <ArrowDown size={16} />
                  </button>
                </div>
              </div>
            )}
            <div className="section-rule" />
            <button
              className="icon-button"
              title="Node JSON"
              aria-label="Node JSON"
              onClick={() => p.openJson(node.name)}
            >
              <Braces size={15} />
            </button>
            {node.name !== doc.graph.root.name &&
              (p.parent && ['loop', 'map'].includes(p.parent.kind) ? (
                <button
                  className="text-button remove-node"
                  onClick={() => p.replaceBody(p.parent!.name)}
                >
                  Replace body
                </button>
              ) : (
                <button className="text-button danger remove-node" onClick={p.remove}>
                  <Trash2 size={15} /> Remove node
                </button>
              ))}
          </>
        ) : (
          <div className="inspector-empty">Select a node to edit its configuration.</div>
        )}
      </div>
    </aside>
  );
}
function ActivitySettings({
  p,
  node,
  binding,
  delivery,
  graphChange,
}: {
  p: InspectorProps;
  node: GraphNode;
  binding: ReturnType<typeof bindingFor>;
  delivery: boolean;
  graphChange: (key: string, value: any) => void;
}) {
  const doc = p.document;
  const [accessError, setAccessError] = useState('');
  useEffect(() => setAccessError(''), [node]);
  const bindingChange = (key: string, value: any) => {
    const next = clone(doc);
    if (value === '') delete next.runtime.nodes[node.name][key];
    else next.runtime.nodes[node.name][key] = value;
    p.edit(next, `binding.${node.name}.${key}`);
  };
  return (
    <>
      {executable(node) && (
        <WorkerControl
          key={`${node.name}-worker`}
          document={doc}
          node={node}
          workers={p.workers}
          onChange={p.edit}
        />
      )}
      {editableActivityRole(node, binding) && (
        <div>
          <button
            type="button"
            className="text-button"
            title={node.kind === 'step' ? 'Make read-only Verifier' : 'Make writing Agent'}
            onClick={() => {
              try {
                p.edit(
                  (node.kind === 'step' ? makeReadOnlyVerifier : makeWritingAgent)(doc, node.name)
                );
                setAccessError('');
              } catch (cause) {
                setAccessError((cause as Error).message);
              }
            }}
          >
            {node.kind === 'step' ? 'Make read-only Verifier' : 'Make writing Agent'}
          </button>
          {accessError && (
            <p className="error-text" role="alert">
              {accessError}
            </p>
          )}
        </div>
      )}
      {executable(node) && !delivery && (
        <>
          <Field label="Instructions">
            <textarea
              rows={7}
              value={node.instructions ?? ''}
              onChange={(e) => graphChange('instructions', e.target.value)}
              placeholder="What should this agent do?"
            />
          </Field>
          {binding?.kind === 'agent' && (
            <>
              <ModelPicker
                label="Model"
                harness={doc.runtime.harness}
                provider={doc.runtime.provider}
                value={binding.model ?? ''}
                onChange={(value) => bindingChange('model', value)}
                openRuntime={p.openRuntime}
              />
              <div className="field-row">
                <Field label="Reasoning">
                  <select
                    value={binding.effort ?? ''}
                    onChange={(e) => bindingChange('effort', e.target.value)}
                  >
                    <option value="">Default</option>
                    {(p.schema?.$defs?.ReasoningEffort?.enum ?? []).map((v: string) => (
                      <option key={v}>{v}</option>
                    ))}
                  </select>
                </Field>
                <Field label="Session">
                  <select
                    value={binding.sessionScope ?? 'execution'}
                    onChange={(e) => bindingChange('sessionScope', e.target.value)}
                  >
                    <option value="execution">Per execution</option>
                    <option value="node_instance">Per node</option>
                  </select>
                </Field>
              </div>
            </>
          )}
          <div className="field-row">
            {node.kind === 'verifier' && editableActivityRole(node, binding) && (
              <Field label="Attempts">
                {[1, 2].includes(node.attempts) ? (
                  <select
                    value={node.attempts}
                    onChange={(event) => graphChange('attempts', Number(event.target.value))}
                  >
                    <option value={1}>1</option>
                    <option value={2}>2</option>
                  </select>
                ) : (
                  <button
                    className="text-button"
                    aria-label="Attempts JSON"
                    onClick={() => p.openJson(node.name)}
                  >
                    {String(node.attempts ?? 'JSON')} <Braces size={14} />
                  </button>
                )}
              </Field>
            )}
            <NumberField
              key={`${node.name}-timeout`}
              node={node.name}
              field="timeoutMs"
              label="Timeout (ms)"
              value={node.timeoutMs}
              optional
              placeholder="No limit"
              onChange={(value) => {
                const next = { ...node };
                if (value === undefined) delete next.timeoutMs;
                else next.timeoutMs = value;
                p.updateNode(next);
              }}
            />
          </div>
        </>
      )}
    </>
  );
}

function NameField({ name, onChange }: { name: string; onChange: (n: string) => void }) {
  const [value, setValue] = useState(name),
    [error, setError] = useState('');
  usePendingText(value !== name);
  function apply() {
    if (value === name) return;
    if (!/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(value) || value.length > 128) {
      setValue(name);
      setError(
        'Name unchanged. Use a letter or underscore, then letters, numbers, dots, dashes or underscores.'
      );
      return;
    }
    try {
      onChange(value);
      setError('');
    } catch (e) {
      setValue(name);
      setError(`Name unchanged. ${(e as Error).message}`);
    }
  }
  return (
    <Field label="Name">
      <input
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onBlur={apply}
        onKeyDown={(e) => {
          if (e.key === 'Enter') apply();
          if (e.key === 'Escape') {
            setValue(name);
            setError('');
          }
        }}
      />
      {error && (
        <small role="alert" className="error-text">
          {error}
        </small>
      )}
    </Field>
  );
}
