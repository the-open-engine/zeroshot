import { useEffect, useRef, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { useAuthoring } from './AuthoringProvider';
import { usePendingText } from './pending-edits';
import { allNodes, findNode, type Document, type GraphNode } from './domain';
import { analyzeWorkflowOutcomes } from './workflow-outcomes';
import { typeSummary } from './schema';

export function OutcomeSettings({
  document,
  node,
  edit,
  selectNode,
}: {
  document: Document;
  node: GraphNode;
  edit: (document: Document) => void;
  selectNode: (name: string) => void;
}) {
  const authoring = useAuthoring();
  const current = useRef(document);
  current.current = document;
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  usePendingText(busy);
  if (node.name !== document.graph.root.name) return null;
  const completions = analyzeWorkflowOutcomes(document).completions.filter(
    (value) => value.boundary === 'run'
  );
  const canComplete =
    node.kind === 'seq' &&
    node.children.length > 0 &&
    !allNodes(node).some((child) => ['succeed', 'fail'].includes(child.kind));
  async function complete() {
    const before = JSON.stringify(document);
    setBusy(true);
    setError('');
    try {
      const next = await authoring.outcome(document, { kind: 'complete', owner: node.name });
      if (!mounted.current) return;
      if (JSON.stringify(current.current) !== before)
        throw new Error('The profile changed. Try again.');
      edit(next);
    } catch (cause) {
      if (mounted.current) setError((cause as Error).message);
    } finally {
      if (mounted.current) setBusy(false);
    }
  }
  return (
    <section className="run-results" aria-label="Run outputs">
      <h3>Outputs</h3>
      {completions.map((completion) => {
        const terminal = findNode(document.graph.root, completion.name)!;
        return (
          <button
            key={completion.name}
            className="group-open"
            onClick={() => selectNode(completion.name)}
          >
            <span>
              {completions.length > 1 ? `${completion.name.replaceAll('_', ' ')} · ` : ''}
              {typeSummary(terminal.output)}
            </span>
            <ChevronRight size={14} />
          </button>
        );
      })}
      {canComplete && (
        <button className="button compact" disabled={busy} onClick={() => void complete()}>
          Add run result
        </button>
      )}
      {error && (
        <p className="modal-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
