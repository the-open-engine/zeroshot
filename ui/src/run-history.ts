import type { EnvironmentDefinition } from './environment-store';
import type { HistoryObservation } from './history-contract';
import { allNodes, children, isGroup, pathTo, type Document, type GraphNode } from './domain';
import { projectControlHistory, type ControlRecord } from './control-history';
import type { WorkflowNodeObservation } from './workflow-observation';

export type ExecutionId = string;
export type RunTerminal =
  { status: 'succeeded'; output: unknown } | { status: 'failed'; reason: string };
type RunTerminalSynopsis =
  { status: 'succeeded' } | { status: 'failed'; reason: string };
export type RuntimeFailure = { atCursor: string; reason: 'runtime_failed' | 'runtime_lost' };
export type RunSummary = {
  runId: string;
  title: string;
  phase: string;
  cursor: string | null;
  terminal?: RunTerminalSynopsis | null;
  runtimeFailure?: RuntimeFailure;
  createdAt?: number | null;
  historyAvailable: boolean;
  observation?: HistoryObservation;
  source?: Record<string, unknown> | null;
  example?: boolean;
};
export type RunDetail = Omit<RunSummary, 'cursor' | 'terminal'> & {
  version: 1;
  projectionVersion: 1;
  cursor: string;
  terminal?: RunTerminal | null;
  graph: Document['graph'];
  runtime: Document['runtime'];
  environment?: EnvironmentDefinition;
  initialInput: unknown;
  history: { initialCursor: string; cursor: string; complete: boolean; limitations: string[] };
};
export type Reference = {
  node: string;
  execution: string | number;
  nodeInstance: string | number;
  runId?: string;
};
export type Outcome = {
  status: string;
  output?: unknown;
  signals?: Record<string, string>;
  diagnostic?: unknown;
  code?: string;
  reason?: string;
  artifacts?: unknown[];
};
export type HistoryEvent = {
  cursor: string;
  event: {
    kind: string;
    reference?: Reference;
    occurrence?: { node: string; mapIndices: number[] };
    attempt?: number;
    input?: unknown;
    completion?: { reference: Reference; outcome: Outcome };
    execution?: string | number | null;
    timestamp?: number;
    stream?: string;
    line?: string;
    reason?: string;
    result?: RunTerminal;
    usage?: Record<string, number> | null;
  };
};
export type HistoryPage = {
  observation?: HistoryObservation;
  events: HistoryEvent[];
  nextCursor: string;
  headCursor: string;
  complete: boolean;
  finished?: boolean;
  runtimeFailure?: RuntimeFailure;
  control?: ControlRecord[];
  controlError?: string;
};

/** Current runtime status is separate from the events available for replay. */
export function finishRunHistory(
  run: RunDetail,
  events: readonly HistoryEvent[],
  page: Pick<HistoryPage, 'headCursor' | 'runtimeFailure' | 'observation'>
): RunDetail {
  let recordedTerminal: RunTerminal | undefined;
  for (let index = events.length - 1; index >= 0; index--) {
    if (events[index].event.kind === 'terminal') {
      recordedTerminal = events[index].event.result;
      break;
    }
  }
  const runtimeFailure = recordedTerminal ? undefined : (page.runtimeFailure ?? run.runtimeFailure);
  return {
    ...run,
    observation: page.observation ?? run.observation,
    phase: 'finished',
    cursor: page.headCursor,
    terminal:
      recordedTerminal ??
      (runtimeFailure ? { status: 'failed', reason: runtimeFailure.reason } : run.terminal),
    runtimeFailure,
    history: {
      ...run.history,
      cursor: page.headCursor,
      complete: recordedTerminal ? true : runtimeFailure ? false : run.history.complete,
    },
  };
}
export type NodeState = 'idle' | 'running' | 'succeeded' | 'failed' | 'skipped';
export type Invocation = {
  id: ExecutionId;
  node: string;
  instance: string;
  attempt: number;
  mapIndices: number[];
  visit: number;
  start: number;
  end?: number;
  input: unknown;
  outcome?: Outcome;
  state: NodeState;
  reason?: string;
  logs: { index: number; timestamp?: number; stream: string; text: string }[];
  usage?: Record<string, number> | null;
};
export type Moment = {
  index: number;
  cursor: string;
  label: string;
  node?: string;
  state?: NodeState;
};
export const humanize = (value: string) =>
  value
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/[_-]+/g, ' ')
    .replace(/^\w/, (c) => c.toUpperCase());
export const stateLabel = (state: NodeState) =>
  ({
    idle: 'Not reached',
    running: 'Running',
    succeeded: 'Completed',
    failed: 'Failed',
    skipped: 'Stopped',
  })[state];
export function eventNode(event: HistoryEvent['event']): string | undefined {
  return event.reference?.node ?? event.completion?.reference.node;
}
export function eventLabel(event: HistoryEvent['event']): string {
  const node = eventNode(event);
  switch (event.kind) {
    case 'run_started':
      return 'Run started';
    case 'node_started':
      return `${humanize(node ?? 'Node')} started`;
    case 'node_completed': {
      const outcome = event.completion?.outcome;
      if (outcome?.status === 'error')
        return `${humanize(node ?? 'Node')} · ${outcome.code ?? 'failed'}`;
      const signals = Object.values(outcome?.signals ?? {});
      return `${humanize(node ?? 'Node')} · ${signals.length ? signals.join(', ') : 'completed'}`;
    }
    case 'execution_voided':
      return `${humanize(node ?? 'Node')} stopped`;
    case 'force_stop_requested':
      return 'Stop requested';
    case 'terminal':
      return event.result?.status === 'failed'
        ? `Run failed · ${event.result.reason}`
        : 'Run completed';
    case 'safe_log':
      return 'Recorded output';
    default:
      return humanize(event.kind);
  }
}

/** Timeline stops are durable lifecycle facts. Logs between stops belong to the next stop. */
export function historyMoments(events: readonly HistoryEvent[], pinnedIndex?: number): Moment[] {
  const moments: Moment[] = [{ index: -1, cursor: 'v2:0', label: 'Before the run' }];
  events.forEach(({ event, cursor }, index) => {
    if (['safe_log', 'token_usage_observed'].includes(event.kind) && index !== pinnedIndex) return;
    moments.push({
      index,
      cursor,
      label: eventLabel(event),
      node: eventNode(event),
      state:
        event.kind === 'node_started'
          ? 'running'
          : event.completion?.outcome.status === 'error'
            ? 'failed'
            : undefined,
    });
  });
  if (events.length && moments.at(-1)?.index !== events.length - 1)
    moments.push({
      index: events.length - 1,
      cursor: events.at(-1)!.cursor,
      label: 'Latest recorded output',
    });
  return moments;
}

/** Replay a node's activity at the same checkpoints as the run, including drained output. */
export function nodeReplayPositions(
  document: Document,
  moments: readonly Moment[],
  events: readonly HistoryEvent[],
  name: string,
  control: readonly ControlRecord[] = []
) {
  const node = allNodes(document.graph.root).find((node) => node.name === name);
  if (!node || moments.length < 2) return [];
  if (name === document.graph.root.name) return moments.map((_, position) => position);
  const names = descendantNames(node);
  const executions = new Map<string, string>();
  const positions = new Set<number>();
  let position = 1;
  events.forEach(({ event }, index) => {
    while (position < moments.length && moments[position].index < index) position++;
    if (event.kind === 'node_started' && event.reference)
      executions.set(identity(event.reference.execution), event.reference.node);
    const name =
      eventNode(event) ??
      (event.execution == null ? undefined : executions.get(identity(event.execution)));
    if (name && names.has(name) && position < moments.length) positions.add(position);
  });
  const selectedCursors = new Set(
    control.filter((record) => names.has(record.node)).map((record) => record.cursor)
  );
  moments.forEach((moment, index) => {
    if (selectedCursors.has(moment.cursor)) positions.add(index);
  });
  return [...positions].sort((a, b) => a - b);
}

export function nextReplayPosition(position: number, total: number, scope?: readonly number[]) {
  return scope
    ? scope.find((next) => next > position)
    : position + 1 < total
      ? position + 1
      : undefined;
}

/** Parallel executions can finish in a different order from their starts. */
export function currentInvocation(
  invocations: readonly Invocation[],
  events: readonly HistoryEvent[],
  through: number
) {
  const byId = new Map(invocations.map((invocation) => [invocation.id, invocation]));
  for (let index = Math.min(through, events.length - 1); index >= 0; index--) {
    const event = events[index].event;
    const execution =
      event.reference?.execution ?? event.completion?.reference.execution ?? event.execution;
    const invocation = execution == null ? undefined : byId.get(identity(execution));
    if (invocation) return invocation;
  }
  return invocations.at(-1);
}

/** Never consult the final snapshot while projecting an earlier cursor. */
export function projectHistory(
  document: Document,
  events: readonly HistoryEvent[],
  through: number,
  control: readonly ControlRecord[] = []
) {
  const invocations: Invocation[] = [];
  const byId = new Map<string, Invocation>();
  const latestVisit = new Map<string, Invocation>();
  const references = new Map<string, Reference>();
  const systemLogs: Invocation['logs'] = [];
  const projection = { invocations, byId, latestVisit, references, systemLogs };
  let terminal: RunTerminal | undefined;
  for (let index = 0; index <= Math.min(through, events.length - 1); index++) {
    const event = events[index].event;
    if (event.kind === 'terminal') terminal = event.result;
    else readHistoryEvent(event, index, projection);
  }
  const nodes: Record<string, WorkflowNodeObservation> = Object.create(null);
  for (const node of allNodes(document.graph.root)) {
    const descendants = new Set(allNodes(node).map((child) => child.name));
    const executions = invocations.filter((invocation) => descendants.has(invocation.node));
    const latestByOccurrence = new Map<string, Invocation>();
    for (const invocation of executions)
      latestByOccurrence.set(JSON.stringify([invocation.node, invocation.mapIndices]), invocation);
    const latest = [...latestByOccurrence.values()];
    // A settled descendant does not prove a group finished: more branches, map items or
    // loop visits may follow. Only active descendants provide a current group observation.
    // Structural terminal nodes have no dispatch record, so do not label them unreached.
    if (isGroup(node)) {
      if (executions.some((invocation) => invocation.state === 'running')) {
        nodes[node.name] = { state: 'running', count: executions.length };
      } else if (executions.length) {
        nodes[node.name] = { state: 'recorded', count: executions.length };
      }
      continue;
    }
    if (!['step', 'verifier'].includes(node.kind)) continue;
    const state: NodeState = latest.some((i) => i.state === 'running')
      ? 'running'
      : latest.some((i) => i.state === 'failed')
        ? 'failed'
        : latest.some((i) => i.state === 'skipped')
          ? 'skipped'
          : latest.length
            ? 'succeeded'
            : 'idle';
    const own = invocations.filter((i) => i.node === node.name);
    const signals = own.at(-1)?.outcome?.signals;
    nodes[node.name] = {
      state,
      count: executions.length,
      detail:
        signals &&
        Object.entries(signals)
          .map(([key, value]) => `${humanize(key)}: ${value}`)
          .join(' · '),
    };
  }
  const controls = projectControlHistory(control, events, through);
  const byControlNode = new Map<string, typeof controls>();
  for (const visit of controls) {
    const visits = byControlNode.get(visit.node) ?? [];
    visits.push(visit);
    byControlNode.set(visit.node, visits);
  }
  for (const [name, visits] of byControlNode) {
    const latest = new Map<string, (typeof controls)[number]>();
    for (const visit of visits) latest.set(JSON.stringify(visit.mapIndices), visit);
    const current = [...latest.values()];
    const state = current.some((visit) => visit.state === 'entered')
      ? 'running'
      : current.some((visit) => visit.state === 'failed')
        ? 'failed'
        : current.some((visit) => visit.state === 'stopped')
          ? 'skipped'
          : 'succeeded';
    const visit = current.reduce((a, b) => (a.updated > b.updated ? a : b));
    nodes[name] = {
      state,
      count: visits.length,
      countUnit: 'visit',
      detail: visit.branch ? `Selected: ${humanize(visit.branch)}` : visit.detail,
    };
  }
  if (terminal)
    nodes[document.graph.root.name] = {
      ...nodes[document.graph.root.name],
      state: terminal.status === 'succeeded' ? 'succeeded' : 'failed',
    };
  return { invocations, nodes, terminal, systemLogs, controls };
}

type HistoryProjection = {
  invocations: Invocation[];
  byId: Map<string, Invocation>;
  latestVisit: Map<string, Invocation>;
  references: Map<string, Reference>;
  systemLogs: Invocation['logs'];
};

function readHistoryEvent(event: HistoryEvent['event'], index: number, projection: HistoryProjection) {
  const { invocations, byId, latestVisit, references, systemLogs } = projection;
  if (event.kind === 'node_started' && event.reference) {
    const id = identity(event.reference.execution);
    const instance = identity(event.reference.nodeInstance);
    if (byId.has(id)) return;
    const node = event.reference.node;
    const mapIndices = event.occurrence?.mapIndices ?? [];
    const scope = JSON.stringify([node, mapIndices]);
    const previous = latestVisit.get(scope);
    const attempt = event.attempt ?? 1;
    const retry = attempt > 1 && previous?.instance === instance && attempt > previous.attempt;
    const visit = retry ? previous.visit : (previous?.visit ?? 0) + 1;
    const invocation: Invocation = {
      id, node, instance, attempt, mapIndices: [...mapIndices], visit, start: index,
      input: event.input, state: 'running', logs: [],
    };
    latestVisit.set(scope, invocation);
    references.set(id, event.reference);
    byId.set(id, invocation);
    invocations.push(invocation);
  } else if (event.kind === 'node_completed' && event.completion) {
    const reference = event.completion.reference;
    const invocation = byId.get(identity(reference.execution));
    if (invocation && sameReference(references.get(invocation.id), reference) && invocation.end === undefined) {
      invocation.end = index;
      invocation.outcome = event.completion.outcome;
      invocation.state = event.completion.outcome.status === 'error' ? 'failed' : 'succeeded';
    }
  } else if (event.kind === 'execution_voided' && event.reference) {
    const invocation = byId.get(identity(event.reference.execution));
    if (invocation && sameReference(references.get(invocation.id), event.reference) && invocation.end === undefined) {
      invocation.end = index;
      invocation.state = 'skipped';
      invocation.reason = event.reason;
    }
  } else if (event.kind === 'safe_log') {
    const log = { index, timestamp: event.timestamp, stream: event.stream ?? 'output', text: event.line ?? '' };
    const invocation = event.execution == null ? undefined : byId.get(identity(event.execution));
    (invocation?.logs ?? systemLogs).push(log);
  } else if (event.kind === 'token_usage_observed' && event.execution != null) {
    const invocation = byId.get(identity(event.execution));
    if (invocation) invocation.usage = accumulateUsage(invocation.usage, event.usage);
  }
}

/** The backend transports native u64 identities as strings; accept only lossless legacy numbers. */
function identity(value: string | number): string {
  if (typeof value === 'number' && (!Number.isSafeInteger(value) || value < 1)) {
    throw new Error(
      'History contains an imprecise execution identity. Reload with a compatible server.'
    );
  }
  return String(value);
}

function sameReference(expected: Reference | undefined, actual: Reference): boolean {
  return (
    !!expected &&
    expected.node === actual.node &&
    identity(expected.execution) === identity(actual.execution) &&
    identity(expected.nodeInstance) === identity(actual.nodeInstance) &&
    expected.runId === actual.runId
  );
}

function accumulateUsage(
  previous: Invocation['usage'],
  delta: Invocation['usage']
): Invocation['usage'] {
  if (previous === null || delta == null) return null;
  if (!previous) return { ...delta };
  const total: Record<string, number> = {};
  for (const key of new Set([...Object.keys(previous), ...Object.keys(delta)])) {
    const left = previous[key],
      right = delta[key];
    // Missing counters remain unknown; never present a partial sum as complete usage.
    if (typeof left === 'number' && typeof right === 'number') total[key] = left + right;
  }
  return total;
}

export function invocationLabel(invocation: Invocation, document: Document): string {
  const labels = invocation.mapIndices.map(
    (index, level) =>
      `${invocation.mapIndices.length > 1 ? `Map ${level + 1}, item` : 'Item'} ${index + 1}`
  );
  const inLoop = pathTo(document.graph.root, invocation.node).some((node) => node.kind === 'loop');
  labels.push(`${inLoop ? 'Visit' : 'Execution'} ${invocation.visit}`);
  if (invocation.attempt > 1) labels.push(`Attempt ${invocation.attempt}`);
  return labels.join(' · ');
}

/** Report observed branch entries; do not evaluate guards or invent a decision event. */
export function observedRoutes(node: GraphNode, invocations: readonly Invocation[]) {
  if (node.kind !== 'choice') return [];
  const branches = [
    ...node.branches.map((branch: any, index: number) => ({
      node: branch.node as GraphNode,
      label: `Path ${index + 1}`,
    })),
    ...(node.otherwise ? [{ node: node.otherwise as GraphNode, label: 'Otherwise' }] : []),
  ];
  return branches
    .flatMap((branch) => {
      const names = new Set(allNodes(branch.node).map((child) => child.name));
      const visits = invocations.filter((invocation) => names.has(invocation.node));
      return visits.length
        ? [
            {
              name: branch.node.name,
              label: branch.label,
              count: visits.length,
              first: visits[0].start,
            },
          ]
        : [];
    })
    .sort((a, b) => a.first - b.first);
}

export function descendantNames(node: GraphNode): Set<string> {
  return new Set([node.name, ...children(node).flatMap((child) => [...descendantNames(child)])]);
}
