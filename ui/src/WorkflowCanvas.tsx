import { useEffect, useMemo, useRef, useState } from 'react';
import {
  ReactFlow,
  Background,
  Controls,
  Handle,
  Position,
  MarkerType,
  NodeToolbar,
  EdgeLabelRenderer,
  BaseEdge,
  getSmoothStepPath,
  useReactFlow,
  type Node,
  type NodeProps,
  type Edge,
  type EdgeProps,
} from '@xyflow/react';
import {
  Plus,
  Braces,
  SlidersHorizontal,
  LayoutGrid,
  ChevronDown,
  ChevronRight,
  Repeat2,
  Columns3,
  Settings2,
  PanelRightOpen,
  Info,
  Check,
  Circle,
  LoaderCircle,
  X,
  Minus,
  FoldVertical,
  Clock3,
} from 'lucide-react';
import { findNode, pathTo, executable, type Document, type Positions } from './domain';
import { typeSummary } from './schema';
import { nodeOutputFields } from './node-data';
import { nodePresentation, nodeIcons } from './node-presentation';
import type { WorkerOption } from './workers';
import { projectWorkflow, type WorkflowNode, type WorkflowEdge } from './workflow-projection';
import { compactWorkflow } from './workflow-display';
import { arrangeWorkflow } from './workflow-layout';
import {
  updateWorkflowMeasurements,
  withWorkflowDimensions,
  type WorkflowMeasurements,
} from './workflow-measurements';
import {
  collapsedWorkflowGroups,
  revealWorkflowNode,
  type WorkflowNodeObservation,
  type WorkflowObservation,
} from './workflow-observation';
export type { WorkflowNodeObservation, WorkflowObservation } from './workflow-observation';
import './workflow.css';

export type WorkflowAction = {
  kind: 'next' | 'outcome' | 'parallel' | 'repeat' | 'review' | 'append';
  owner: string;
};
type ActivityData = {
  item: WorkflowNode;
  document: Document;
  workers: WorkerOption[];
  action?: (a: WorkflowAction) => void;
  expand: (name: string) => void;
  readOnly: boolean;
  observation?: WorkflowNodeObservation;
  [key: string]: unknown;
};

const observationLabels = {
  idle: 'Not started',
  running: 'Running',
  succeeded: 'Succeeded',
  failed: 'Failed',
  skipped: 'Stopped',
  recorded: 'Recorded activity',
};

function ObservationBadge({ observation }: { observation: WorkflowNodeObservation }) {
  const Icon = {
    idle: Circle,
    running: LoaderCircle,
    succeeded: Check,
    failed: X,
    skipped: Minus,
    recorded: Clock3,
  }[observation.state];
  const showCount = (observation.count ?? 0) >= (observation.state === 'recorded' ? 1 : 2);
  const unit = observation.countUnit ?? 'execution';
  const executions = showCount
    ? `${observation.count} ${unit}${observation.count === 1 ? '' : 's'}`
    : '';
  const title = [observationLabels[observation.state], executions, observation.detail]
    .filter(Boolean)
    .join(' · ');
  return (
    <span
      className={`workflow-observation state-${observation.state}`}
      title={title}
      aria-label={title}
    >
      <Icon size={11} />
      {showCount && <span>{observation.count}</span>}
    </span>
  );
}

function Activity({ data, selected }: NodeProps<Node<ActivityData>>) {
  const { item, document, workers } = data;
  const node = findNode(document.graph.root, item.owner)!;
  const presentation = nodePresentation(document, node, workers);
  const activity = item.role === 'activity';
  const completion = item.role === 'completion';
  const groupEnd = ['join', 'map-end', 'loop-end'].includes(item.role);
  const Icon = activity ? presentation.Icon : (nodeIcons[item.kind] ?? presentation.Icon);
  const terminal = ['succeed', 'fail'].includes(item.kind);
  const junction = ['fork', 'join', 'loop-start', 'loop-end', 'map-end'].includes(item.role);
  const outputFields = nodeOutputFields(node);
  const outputs = outputFields.length
    ? typeSummary({
        kind: 'record',
        fields: Object.fromEntries(outputFields.map((field) => [field.name, field])),
      })
    : node.output?.kind !== 'null' && node.output?.kind
      ? typeSummary(node.output)
      : '';
  return (
    <>
      <NodeToolbar
        isVisible={
          !!selected &&
          !data.readOnly &&
          ((activity && !terminal) ||
            groupEnd ||
            ['decision', 'fork', 'loop-start', 'map-start', 'empty'].includes(item.role))
        }
        position={Position.Bottom}
        offset={8}
      >
        <div className="workflow-node-actions">
          {groupEnd && (
            <button
              aria-label={`Add next after ${item.owner}`}
              onClick={() => data.action?.({ kind: 'next', owner: item.owner })}
            >
              <Plus size={13} /> Add next
            </button>
          )}
          {activity && !terminal && (
            <>
              <button onClick={() => data.action?.({ kind: 'next', owner: item.owner })}>
                <Plus size={13} /> Add next
              </button>
              <button
                title="Run in parallel"
                aria-label={`Run ${item.owner} in parallel`}
                onClick={() => data.action?.({ kind: 'parallel', owner: item.owner })}
              >
                <Columns3 size={14} />
              </button>
              <button
                title={node.kind === 'step' ? 'Repeat until approved' : 'Repeat activity'}
                aria-label={`Repeat ${item.owner}`}
                onClick={() =>
                  data.action?.({
                    kind: node.kind === 'step' ? 'review' : 'repeat',
                    owner: item.owner,
                  })
                }
              >
                <Repeat2 size={14} />
              </button>
            </>
          )}
          {item.role === 'decision' && (
            <button onClick={() => data.action?.({ kind: 'outcome', owner: item.owner })}>
              <Plus size={13} /> Add outcome
            </button>
          )}
          {['fork', 'loop-start', 'map-start', 'empty'].includes(item.role) && (
            <button onClick={() => data.action?.({ kind: 'append', owner: item.owner })}>
              <Plus size={13} /> Add {item.role === 'fork' ? 'parallel activity' : 'activity'}
            </button>
          )}
        </div>
      </NodeToolbar>
      <div
        title={[item.label, item.detail].filter(Boolean).join(' · ')}
        aria-label={item.label}
        className={`workflow-card ${completion ? 'is-completion' : ''} ${junction ? 'is-junction' : ''} role-${item.role} kind-${item.kind} ${selected ? 'chosen' : ''} ${data.observation ? `observed state-${data.observation.state}` : ''}`}
      >
        <Handle
          type="target"
          position={Position.Left}
          isConnectable={!data.readOnly && activity && !terminal}
        />
        {data.observation && <ObservationBadge observation={data.observation} />}
        <div className="workflow-card-heading">
          <Icon size={18} />
          <span>{activity && presentation.delivery ? 'Git delivery' : item.label}</span>
        </div>
        {activity && executable(node) && (
          <small className={!presentation.detail ? 'needs-value' : ''}>
            {presentation.detail || (data.readOnly ? 'Agent' : 'Choose model')}
          </small>
        )}
        {item.detail && !activity && !junction && item.role !== 'decision' && (
          <small title={item.detail}>{item.detail}</small>
        )}
        {activity && executable(node) && (
          <div className="workflow-fields">
            {node.input?.kind && node.input.kind !== 'null' && (
              <div title={typeSummary(node.input)}>
                <b>In</b> {typeSummary(node.input)}
              </div>
            )}
            {outputs && (
              <div title={outputs}>
                <b>Out</b> {outputs}
              </div>
            )}
          </div>
        )}
        {activity && <span className="workflow-node-identity">{node.name}</span>}
        {item.role === 'collapsed' && (
          <button
            className="workflow-expand nodrag nopan"
            aria-label={`Expand ${item.owner}`}
            onClick={(e) => {
              e.stopPropagation();
              data.expand(item.owner);
            }}
          >
            Expand <ChevronRight size={13} />
          </button>
        )}
        <Handle
          type="source"
          position={Position.Right}
          isConnectable={!data.readOnly && activity && !terminal}
        />
      </div>
    </>
  );
}

function Region({ data }: NodeProps) {
  const value = data as any;
  return (
    <div
      className={`workflow-region region-${value.kind}`}
      style={{ width: value.width, height: value.height }}
    >
      <div className="workflow-region-header nodrag nopan">
        <button onClick={value.select}>
          <span>{value.label}</span>
        </button>
        {value.observation && <ObservationBadge observation={value.observation} />}
        <button
          title={`Collapse ${value.owner}`}
          aria-label={`Collapse ${value.owner}`}
          onClick={value.collapse}
        >
          <ChevronDown size={14} />
        </button>
      </div>
    </div>
  );
}

function FlowEdge(props: EdgeProps) {
  const data = props.data as any;
  const [normal, labelX, labelY] = getSmoothStepPath({ ...props, borderRadius: 12, offset: 22 });
  const repeat = data?.repeat;
  const lane = data?.lane ?? Math.max(props.sourceY, props.targetY) + 100;
  const path = repeat
    ? `M ${props.sourceX} ${props.sourceY} L ${props.sourceX + 26} ${props.sourceY} L ${props.sourceX + 26} ${lane} L ${props.targetX - 26} ${lane} L ${props.targetX - 26} ${props.targetY} L ${props.targetX} ${props.targetY}`
    : normal;
  return (
    <>
      <BaseEdge id={props.id} path={path} markerEnd={props.markerEnd} style={props.style} />
      {props.label && (
        <EdgeLabelRenderer>
          <button
            className={`workflow-edge-label nodrag nopan ${repeat ? 'repeat-label' : ''}`}
            style={{
              transform: `translate(-50%, -50%) translate(${repeat ? (props.sourceX + props.targetX) / 2 : labelX}px,${repeat ? lane : labelY}px)`,
            }}
            title={data?.fullLabel ?? String(props.label)}
            aria-label={`${data?.readOnly ? 'Inspect' : 'Edit'} path: ${props.label}`}
            onClick={(event) => {
              event.stopPropagation();
              data?.activate();
            }}
          >
            {String(props.label)}
          </button>
        </EdgeLabelRenderer>
      )}
    </>
  );
}
const nodeTypes = { activity: Activity, region: Region };
const edgeTypes = { workflow: FlowEdge };
const dimensions = (item: WorkflowNode) =>
  ['fork', 'join', 'loop-start', 'loop-end', 'map-end'].includes(item.role)
    ? { width: 28, height: 28 }
    : item.role === 'completion'
      ? { width: 132, height: 40 }
      : item.role === 'activity'
        ? ['succeed', 'fail'].includes(item.kind)
          ? { width: 152, height: 64 }
          : { width: 190, height: 104 }
        : item.role === 'decision'
          ? { width: 164, height: 72 }
          : { width: 190, height: 104 };

export function WorkflowCanvas(p: {
  document: Document;
  selected: string;
  positions: Positions;
  setPositions: (p: Positions) => void;
  workers: WorkerOption[];
  select: (name: string) => void;
  focus?: { name: string; tick: number };
  observation?: WorkflowObservation;
  action?: (a: WorkflowAction) => void;
  editOutcome?: (edge: WorkflowEdge) => void;
  reorder?: (source: string, target: string) => void;
  runtime?: () => void;
  json?: () => void;
  info?: () => void;
  inspectorHidden: boolean;
  showInspector: () => void;
}) {
  const { fitView, setCenter } = useReactFlow();
  const readOnly = !!p.observation;
  const contextKey = p.observation?.key ?? p.document.name;
  const [collapsed, setCollapsed] = useState<Set<string>>(() =>
    readOnly ? collapsedWorkflowGroups(p.document.graph.root) : new Set()
  );
  const [error, setError] = useState('');
  const layoutRevision = useRef(0);
  const lastFocus = useRef<number | undefined>(undefined);
  const flowElement = useRef<HTMLDivElement>(null);
  const projection = useMemo(
    () => compactWorkflow(projectWorkflow(p.document, collapsed)),
    [p.document, collapsed]
  );
  const signature = JSON.stringify([
    projection.nodes.map((n) => [n.id, dimensions(n)]),
    projection.edges.map((e) => [e.source, e.target, e.repeat]),
    projection.regions.map((region) => [region.id, region.kind, region.nodeIds]),
  ]);
  // Manual positions belong to a particular visible topology; collapsing or wrapping
  // content must not reuse coordinates from a different arrangement of scopes.
  const layoutKey = useMemo(() => {
    let first = 2166136261,
      second = 5381;
    for (let index = 0; index < signature.length; index++) {
      first = Math.imul(first ^ signature.charCodeAt(index), 16777619);
      second = Math.imul(second, 33) ^ signature.charCodeAt(index);
    }
    return `workflow-layout-v2:${signature.length}:${first >>> 0}:${second >>> 0}:`;
  }, [signature]);
  const positionsRef = useRef(p.positions);
  positionsRef.current = p.positions;
  const focusRef = useRef(p.focus);
  focusRef.current = p.focus;
  const [layoutPositions, setLayoutPositions] = useState<Positions>({});
  const [layoutRegions, setLayoutRegions] = useState<
    Record<string, { x: number; y: number; width: number; height: number }>
  >({});
  const [dragPositions, setDragPositions] = useState<Positions>({});
  const [measurements, setMeasurements] = useState<WorkflowMeasurements>({});
  const [layoutBusy, setLayoutBusy] = useState(true);
  const dims = Object.fromEntries(projection.nodes.map((n) => [n.id, dimensions(n)]));
  const savedPositions = Object.fromEntries(
    projection.nodes.flatMap((node) => {
      const value = p.positions[layoutKey + node.id];
      return value ? [[node.id, value]] : [];
    })
  );
  const currentPositions = { ...layoutPositions, ...savedPositions, ...dragPositions };
  function collapse(name: string) {
    setCollapsed((value) => new Set(value).add(name));
  }
  function expand(name: string) {
    setCollapsed((value) => {
      const next = new Set(value);
      next.delete(name);
      return next;
    });
  }
  async function layout(reset = false) {
    const revision = ++layoutRevision.current;
    setLayoutBusy(true);
    try {
      const next = await arrangeWorkflow(projection, dims);
      if (revision !== layoutRevision.current) return;
      setLayoutPositions(next.positions);
      setLayoutRegions(next.regions);
      if (reset)
        p.setPositions({
          ...positionsRef.current,
          ...Object.fromEntries(
            Object.entries(next.positions).map(([id, value]) => [layoutKey + id, value])
          ),
        });
      setError('');
      if (reset || !focusRef.current || lastFocus.current === focusRef.current.tick)
        requestAnimationFrame(() =>
          requestAnimationFrame(() => fitView({ padding: 0.12, maxZoom: 0.9, duration: 180 }))
        );
    } catch (cause) {
      if (revision === layoutRevision.current) setError((cause as Error).message);
    } finally {
      if (revision === layoutRevision.current) setLayoutBusy(false);
    }
  }
  useEffect(() => {
    setCollapsed(readOnly ? collapsedWorkflowGroups(p.document.graph.root) : new Set());
    setDragPositions({});
    lastFocus.current = undefined;
  }, [contextKey, readOnly]);
  useEffect(() => {
    void layout();
    return () => {
      layoutRevision.current++;
    };
  }, [signature, contextKey]);
  useEffect(() => setDragPositions({}), [layoutKey]);
  useEffect(() => {
    if (!readOnly || !flowElement.current) return;
    let previous = '';
    let timer: ReturnType<typeof setTimeout> | undefined;
    const observer = new ResizeObserver(([entry]) => {
      const size = `${Math.round(entry.contentRect.width)}:${Math.round(entry.contentRect.height)}`;
      if (size === previous) return;
      const initialized = !!previous;
      previous = size;
      if (!initialized || !entry.contentRect.width || !entry.contentRect.height) return;
      clearTimeout(timer);
      timer = setTimeout(() => void fitView({ padding: 0.12, maxZoom: 0.9, duration: 180 }), 120);
    });
    observer.observe(flowElement.current);
    return () => {
      observer.disconnect();
      clearTimeout(timer);
    };
  }, [readOnly, fitView]);
  useEffect(() => {
    if (!p.focus || lastFocus.current === p.focus.tick) return;
    const ancestors = pathTo(p.document.graph.root, p.focus.name)
      .slice(0, -1)
      .map((node) => node.name);
    const hidden = projection.nodes.find(
      (n) => n.role === 'collapsed' && ancestors.includes(n.owner)
    );
    if (hidden && !projection.nodes.some((n) => n.owner === p.focus!.name)) {
      setCollapsed((value) =>
        readOnly ? revealWorkflowNode(value, p.document.graph.root, p.focus!.name) : new Set()
      );
      return;
    }
    if (layoutBusy) return;
    const target = projection.nodes.find((n) => n.owner === p.focus!.name);
    if (target) {
      const position = currentPositions[target.id];
      if (position) {
        setCenter(position.x + dims[target.id].width / 2, position.y + dims[target.id].height / 2, {
          zoom: 0.95,
          duration: 220,
        });
      }
    } else {
      const region = projection.regions.find((value) => value.owner === p.focus!.name);
      const geometry = region && layoutRegions[region.id];
      if (geometry)
        setCenter(geometry.x + geometry.width / 2, geometry.y + geometry.height / 2, {
          zoom: 0.8,
          duration: 220,
        });
    }
    // Flattened sequence owners have settings but no card to center on.
    lastFocus.current = p.focus.tick;
  }, [p.focus, signature, layoutBusy]);

  const activities: Node[] = projection.nodes.map((item) =>
    withWorkflowDimensions(
      {
        id: item.id,
        type: 'activity',
        position: currentPositions[item.id] ?? { x: 0, y: 0 },
        selected: p.selected === item.owner && !p.inspectorHidden,
        data: {
          item,
          document: p.document,
          workers: p.workers,
          action: p.action,
          expand,
          readOnly,
          observation: p.observation?.nodes[item.owner],
        },
      },
      dimensions(item),
      measurements
    )
  );
  const boxes = projection.regions
    .filter((r) => r.nodeIds.length)
    .map((region) => {
      const members = activities.filter((n) => region.nodeIds.includes(n.id));
      const bounds = (values: Positions) => ({
        left: Math.min(...members.map((n) => (values[n.id] ?? n.position).x)),
        top: Math.min(...members.map((n) => (values[n.id] ?? n.position).y)),
        right: Math.max(...members.map((n) => (values[n.id] ?? n.position).x + dims[n.id].width)),
        bottom: Math.max(...members.map((n) => (values[n.id] ?? n.position).y + dims[n.id].height)),
      });
      const original = bounds(layoutPositions),
        current = bounds(currentPositions),
        geometry = layoutRegions[region.id];
      const x = geometry ? geometry.x + current.left - original.left : current.left - 26;
      const y = geometry ? geometry.y + current.top - original.top : current.top - 56;
      const right = geometry
        ? geometry.x + geometry.width + current.right - original.right
        : current.right + 26;
      const bottom = geometry
        ? geometry.y + geometry.height + current.bottom - original.bottom
        : current.bottom + 26;
      return { region, x, y, width: right - x, height: bottom - y, bottom };
    })
    .filter((box) => Number.isFinite(box.x))
    .sort((a, b) => b.width * b.height - a.width * a.height);
  const regions: Node[] = boxes.map((box, index) =>
    withWorkflowDimensions(
      {
        id: box.region.id,
        type: 'region',
        position: { x: box.x, y: box.y },
        style: { zIndex: -20 + index },
        draggable: false,
        selectable: false,
        connectable: false,
        data: {
          ...box.region,
          observation: p.observation?.nodes[box.region.owner],
          width: box.width,
          height: box.height,
          select: () => p.select(box.region.owner),
          collapse: () => collapse(box.region.owner),
        },
      },
      { width: box.width, height: box.height },
      measurements
    )
  );
  const edges: Edge[] = projection.edges.map((edge, index) => ({
    ...edge,
    label: edge.shortLabel ?? edge.label,
    type: 'workflow',
    markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16 },
    style: {
      stroke: edge.secondary ? 'var(--muted)' : edge.repeat ? 'var(--rust-text)' : 'var(--edge)',
      strokeWidth: edge.repeat ? 1.5 : 1.2,
      strokeDasharray: edge.secondary ? '4 5' : undefined,
    },
    data: {
      ...edge,
      readOnly,
      lane:
        (boxes.find((b) => b.region.owner === edge.owner)?.bottom ??
          Math.max(...activities.map((n) => n.position.y + dims[n.id].height))) +
        28 +
        (index % 3) * 12,
      activate: () =>
        !readOnly && (edge.branchIndex !== undefined || edge.otherwise)
          ? p.editOutcome?.(edge)
          : edge.owner && p.select(edge.owner),
    },
  }));
  const selectedNode = p.inspectorHidden ? undefined : findNode(p.document.graph.root, p.selected);
  return (
    <section
      className={`canvas-region workflow-canvas ${readOnly ? 'workflow-readonly' : ''}`}
      aria-label={readOnly ? 'Run graph' : 'Workflow editor'}
    >
      <div className="canvas-toolbar">
        <button className="button compact" onClick={() => p.select(p.document.graph.root.name)}>
          {readOnly ? <Info size={14} /> : <Settings2 size={14} />}
          {readOnly ? 'Run overview' : 'Run settings'}
        </button>
        <div className="canvas-actions">
          {readOnly ? (
            <button
              className="icon-button"
              title="Collapse all groups"
              aria-label="Collapse all groups"
              onClick={() => setCollapsed(collapsedWorkflowGroups(p.document.graph.root))}
            >
              <FoldVertical size={17} />
            </button>
          ) : (
            <>
              <button
                className="icon-button"
                title="Runtime settings · whole graph"
                aria-label="Runtime settings"
                onClick={p.runtime}
              >
                <SlidersHorizontal size={17} />
              </button>
              <button
                className="icon-button"
                title="Graph JSON"
                aria-label="Graph JSON"
                onClick={p.json}
              >
                <Braces size={17} />
              </button>
              <button
                className="icon-button"
                title="Workflow defaults"
                aria-label="Workflow defaults"
                onClick={p.info}
              >
                <Info size={17} />
              </button>
              <button
                className="icon-button"
                title="Arrange workflow"
                aria-label="Arrange workflow"
                onClick={() => void layout(true)}
              >
                <LayoutGrid size={17} />
              </button>
            </>
          )}
          {p.inspectorHidden && (
            <button className="icon-button" aria-label="Open inspector" onClick={p.showInspector}>
              <PanelRightOpen size={17} />
            </button>
          )}
          {!readOnly && (
            <button
              className="button compact"
              disabled={!!selectedNode && ['succeed', 'fail'].includes(selectedNode.kind)}
              onClick={() =>
                p.action?.({
                  kind:
                    selectedNode && executable(selectedNode)
                      ? 'next'
                      : selectedNode?.kind === 'choice'
                        ? 'outcome'
                        : 'append',
                  owner:
                    selectedNode &&
                    (['seq', 'choice', 'loop', 'map', 'par'].includes(selectedNode.kind) ||
                      executable(selectedNode))
                      ? selectedNode.name
                      : p.document.graph.root.name,
                })
              }
            >
              <Plus size={15} /> Add activity
            </button>
          )}
        </div>
      </div>
      <div ref={flowElement} className={`flow ${layoutBusy ? 'workflow-arranging' : ''}`}>
        <ReactFlow
          nodes={[...regions, ...activities]}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          onNodeClick={(_, node) => {
            if (node.type === 'activity') p.select((node.data as ActivityData).item.owner);
          }}
          onNodeDragStop={(_, node) => {
            if (readOnly) return;
            p.setPositions({ ...positionsRef.current, [layoutKey + node.id]: node.position });
            setDragPositions((value) => {
              const next = { ...value };
              delete next[node.id];
              return next;
            });
          }}
          onNodesChange={(changes) => {
            setMeasurements((previous) => updateWorkflowMeasurements(previous, changes));
            const moved = readOnly
              ? []
              : changes.filter((change) => change.type === 'position' && change.position);
            if (moved.length)
              setDragPositions((value) => {
                const next = { ...value };
                for (const change of moved)
                  if (change.type === 'position' && change.position)
                    next[change.id] = change.position;
                return next;
              });
          }}
          onConnect={(connection) => {
            if (readOnly) return;
            const source = projection.nodes.find((n) => n.id === connection.source);
            const target = projection.nodes.find((n) => n.id === connection.target);
            if (source && target) p.reorder?.(source.owner, target.owner);
          }}
          nodesDraggable={!readOnly}
          nodesConnectable={!readOnly}
          minZoom={0.15}
          maxZoom={1.8}
          deleteKeyCode={null}
          edgesReconnectable={false}
          proOptions={{ hideAttribution: true }}
          ariaLabelConfig={{
            'controls.fitView.ariaLabel': 'Fit workflow',
            'controls.zoomIn.ariaLabel': 'Zoom in',
            'controls.zoomOut.ariaLabel': 'Zoom out',
          }}
        >
          <Background gap={24} size={1} color="var(--grid)" />
          <Controls showInteractive={false} />
        </ReactFlow>
        {error && (
          <div className="canvas-error" role="alert">
            {error}
          </div>
        )}
      </div>
    </section>
  );
}
