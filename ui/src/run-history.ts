import type { EnvironmentDefinition } from './environment-store';
import type { FailureContext } from './failure-context';
import type { HistoryObservation } from './history-contract';
import {
  allNodes,
  children,
  executable,
  isGroup,
  pathTo,
  type Document,
  type GraphNode,
} from './domain';
import {
  controlState,
  projectControlHistory,
  settleControlVisits,
  type ControlRecord,
  type ControlVisit,
} from './control-history';
import type { WorkflowNodeObservation } from './workflow-observation';

export type ExecutionId = string;
export type RunTerminal =
  | { status: 'succeeded'; output: unknown }
  | { status: 'failed'; reason: string; context?: FailureContext | null };
type RunTerminalSynopsis = { status: 'succeeded' } | { status: 'failed'; reason: string };
export type RuntimeFailure = {
  atCursor: string;
  reason: 'runtime_failed' | 'runtime_lost';
  context?: FailureContext | null;
};
export type RunSummary = {
  runId: string;
  title: string;
  phase: string;
  suspendRequested?: boolean;
  suspension?: {
    reason: 'command' | 'wait' | 'request';
    node: string;
    createdAt: number;
    wait?: { dueAt: number };
    request?: {
      id: string;
      text: string;
      signals: Record<string, string[]>;
      output: unknown;
      expiresAt: number;
    };
  };
  workspaceRecovery?: { successorRunId?: string };
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
  metadata?: { deliveryReceipt?: unknown; tokenUsage?: unknown };
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
  /**
   * Unix milliseconds when the ledger recorded this event; one value per append batch and never
   * decreasing within a run. Absent when unknown: older servers and ledgers, or rows recorded
   * before the ledger kept times. Order is defined by `cursor` alone.
   */
  recordedAt?: number;
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
    suspension?: RunSummary['suspension'];
    result?: RunTerminal;
    usage?: Record<string, number> | null;
    vector?: VectorRecord;
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

function runtimeFailureTerminal(failure: RuntimeFailure): RunTerminal {
  const terminal: Extract<RunTerminal, { status: 'failed' }> = {
    status: 'failed',
    reason: failure.reason,
  };
  if (failure.context) terminal.context = failure.context;
  return terminal;
}

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
  const suspended =
    [...events].reverse().find(({ event }) => event.kind === 'suspended')?.event.suspension ??
    (run.phase === 'suspended' ? run.suspension : undefined);
  const runtimeFailure = recordedTerminal ? undefined : (page.runtimeFailure ?? run.runtimeFailure);
  return {
    ...run,
    observation: page.observation ?? run.observation,
    phase: suspended && !recordedTerminal ? 'suspended' : 'finished',
    suspension: suspended ?? run.suspension,
    suspendRequested: false,
    cursor: page.headCursor,
    terminal:
      recordedTerminal ?? (runtimeFailure ? runtimeFailureTerminal(runtimeFailure) : run.terminal),
    runtimeFailure,
    history: {
      ...run.history,
      cursor: page.headCursor,
      complete:
        recordedTerminal || suspended ? true : runtimeFailure ? false : run.history.complete,
    },
  };
}
export type NodeState = 'idle' | 'running' | 'suspended' | 'succeeded' | 'failed' | 'skipped';
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
  /** Settled or held in an earlier run and carried into this one when it continued. */
  imported?: boolean;
  /**
   * Ledger times of this execution's start and settlement. Carried-over executions have no start:
   * their recorded time is when they were imported, not when they ran.
   */
  startedAt?: number;
  endedAt?: number;
  /** The question this execution asked a person, and how it was resolved. */
  request?: PersonRequest;
};
/** A question a request node put to a person, as recorded in the run's vector history. */
export type PersonRequest = {
  id: string;
  node: string;
  text: string;
  /** The labels each answer signal allows. */
  signals: Record<string, string[]>;
  output: unknown;
  createdAt?: number;
  expiresAt: number;
  /** The run that recorded the question. */
  runId?: string;
  resolution?:
    | {
        kind: 'answer';
        signals: Record<string, string>;
        output: unknown;
        at?: number;
        runId?: string;
      }
    | { kind: 'expiry'; at?: number; runId?: string };
};
/** Where this run continued an earlier one, at the index of its continuation record. */
export type RunHandoff = { kind: 'continued'; index: number; from: string };
type PriorExecution = {
  execution: string | number;
  node_instance: string | number;
  occurrence: { node: string; mapIndices: number[] };
  attempt?: number;
  input?: unknown;
  state: 'Active' | { Settled: { outcome: Outcome } } | { Voided: { reason: string } };
};
type VectorRecord = {
  id: string;
  runId?: string;
  createdAt?: number;
  kind: string;
  node?: string;
  text?: string;
  signals?: Record<string, string[]>;
  output?: unknown;
  expiresAt?: number;
  requestId?: string;
  answer?: { signals: Record<string, string>; output: unknown };
  reason?: string;
  resumedFrom?: string;
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
    suspended: 'Suspended',
    succeeded: 'Completed',
    failed: 'Failed',
    skipped: 'Stopped',
  })[state];
export function eventNode(event: HistoryEvent['event']): string | undefined {
  return (
    event.reference?.node ??
    event.completion?.reference.node ??
    priorExecution(event)?.occurrence.node ??
    (event.kind === 'vector' ? event.vector?.node : undefined)
  );
}
function priorExecution(event: HistoryEvent['event']): PriorExecution | undefined {
  if (event.kind !== 'prior_execution') return undefined;
  const value = (event as { execution?: unknown }).execution;
  return value && typeof value === 'object' ? (value as PriorExecution) : undefined;
}
/**
 * What an execution decided, as recorded; a person's request reads as answered, expired or
 * waiting. A recorded answer or expiry outranks the suspended state it has not yet settled.
 */
export function executionDecision(invocation: Invocation): string | undefined {
  const resolution = invocation.request?.resolution;
  if (resolution?.kind === 'expiry') return 'No answer in time';
  if (resolution?.kind === 'answer' && invocation.state === 'suspended') {
    const chosen = Object.values(resolution.signals);
    return chosen.length ? chosen.map(humanize).join(', ') : 'Answered';
  }
  if (invocation.state === 'failed')
    return humanize(invocation.outcome?.code ?? invocation.outcome?.reason ?? 'failed');
  if (invocation.state === 'suspended')
    return invocation.request ? 'Waiting for an answer' : 'Waiting';
  const signals = Object.values(invocation.outcome?.signals ?? {});
  return signals.length ? signals.map(humanize).join(', ') : undefined;
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
    case 'wait_deferred':
    case 'request_deferred':
      return `${humanize(node ?? 'Wait')} suspended`;
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
    case 'prior_execution':
      return `${humanize(node ?? 'Node')} carried over from an earlier run`;
    case 'vector':
      return vectorLabel(event.vector);
    default:
      return humanize(event.kind);
  }
}

function vectorLabel(vector: VectorRecord | undefined): string {
  switch (vector?.kind) {
    case 'request':
      return `${humanize(vector.node ?? 'Request')} asked a person`;
    case 'answer':
      return 'A person answered';
    case 'expiry':
      return 'The request expired without an answer';
    case 'suspension':
      return 'Run paused';
    case 'continuation':
      return 'Continued from an earlier run';
    default:
      return 'Run record';
  }
}
/** Imported work and bookkeeping records are context, not stops worth replaying one by one. */
const quietVectors = new Set(['suspend_request', 'wait']);

/** Timeline stops are durable lifecycle facts. Logs between stops belong to the next stop. */
export function historyMoments(events: readonly HistoryEvent[], pinnedIndex?: number): Moment[] {
  const moments: Moment[] = [{ index: -1, cursor: 'v2:0', label: 'Before the run' }];
  events.forEach(({ event, cursor }, index) => {
    if (
      (['safe_log', 'token_usage_observed', 'prior_execution'].includes(event.kind) ||
        (event.kind === 'vector' && quietVectors.has(event.vector?.kind ?? ''))) &&
      index !== pinnedIndex
    )
      return;
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
  const requests: PersonRequest[] = [];
  const handoffs: RunHandoff[] = [];
  const projection: HistoryProjection = {
    invocations,
    byId,
    latestVisit,
    references,
    systemLogs,
    requests,
    handoffs,
    unowned: [],
  };
  let terminal: RunTerminal | undefined;
  /**
   * The run's first recorded time and the latest one at this position, when the ledger kept times.
   * Events from before the ledger recorded times leave the start unknown rather than estimated.
   */
  const clock: { first?: number; at?: number } = {};
  let untimedStart = false;
  for (let index = 0; index <= Math.min(through, events.length - 1); index++) {
    const { event, recordedAt } = events[index];
    if (recordedAt === undefined) untimedStart ||= clock.at === undefined;
    else {
      if (!untimedStart) clock.first ??= recordedAt;
      clock.at = recordedAt;
    }
    if (event.kind === 'terminal') terminal = event.result;
    else readHistoryEvent(event, index, projection, recordedAt);
  }
  const controls = settleControlVisits(
    document.graph.root,
    projectControlHistory(control, events, through)
  );
  const byControlNode = new Map<string, typeof controls>();
  for (const visit of controls) {
    const visits = byControlNode.get(visit.node) ?? [];
    visits.push(visit);
    byControlNode.set(visit.node, visits);
  }
  const nodes: Record<string, WorkflowNodeObservation> = Object.create(null);
  for (const node of allNodes(document.graph.root)) {
    const visits = byControlNode.get(node.name) ?? [];
    if (executable(node)) {
      nodes[node.name] = leafObservation(
        invocations.filter((invocation) => invocation.node === node.name),
        visits,
        !terminal
      );
    } else if (visits.length) {
      nodes[node.name] = controlObservation(visits);
    } else if (isGroup(node)) {
      // Without a structural record, only active descendants establish a current group state.
      const descendants = new Set(allNodes(node).map((child) => child.name));
      const executions = invocations.filter((invocation) => descendants.has(invocation.node));
      if (executions.some((invocation) => invocation.state === 'running'))
        nodes[node.name] = { state: 'running', count: executions.length };
      else if (executions.length)
        nodes[node.name] = { state: 'recorded', count: executions.length };
    }
  }
  if (terminal)
    nodes[document.graph.root.name] = {
      ...nodes[document.graph.root.name],
      state: terminal.status === 'succeeded' ? 'succeeded' : 'failed',
    };
  return { invocations, nodes, terminal, systemLogs, controls, requests, handoffs, clock };
}

type HistoryProjection = {
  invocations: Invocation[];
  byId: Map<string, Invocation>;
  latestVisit: Map<string, Invocation>;
  references: Map<string, Reference>;
  systemLogs: Invocation['logs'];
  requests: PersonRequest[];
  handoffs: RunHandoff[];
  /** The execution whose request deferral awaits the question it asked. */
  asking?: Invocation;
  /** Carried-over questions recorded before the execution that asked them. */
  unowned: PersonRequest[];
};

function readHistoryEvent(
  event: HistoryEvent['event'],
  index: number,
  projection: HistoryProjection,
  recordedAt?: number
) {
  const { byId, systemLogs } = projection;
  const prior = priorExecution(event);
  if (event.kind === 'node_started' && event.reference) {
    const invocation = startInvocation(projection, index, {
      reference: event.reference,
      mapIndices: event.occurrence?.mapIndices ?? [],
      attempt: event.attempt ?? 1,
      input: event.input,
    });
    if (invocation) invocation.startedAt = recordedAt;
  } else if (prior) {
    readPriorExecution(prior, index, projection);
  } else if (event.kind === 'vector' && event.vector) {
    readVector(event.vector, index, projection);
  } else if (event.kind === 'suspended' && event.suspension?.request) {
    const { request, node, createdAt } = event.suspension;
    recordRequest(projection, { ...request, node, createdAt });
  } else if (event.kind === 'node_completed' && event.completion) {
    const invocation = openInvocation(projection, event.completion.reference);
    if (!invocation) return;
    invocation.end = index;
    invocation.endedAt = recordedAt;
    invocation.outcome = event.completion.outcome;
    invocation.state = event.completion.outcome.status === 'error' ? 'failed' : 'succeeded';
  } else if (
    ['execution_voided', 'wait_deferred', 'request_deferred'].includes(event.kind) &&
    event.reference
  ) {
    const invocation = openInvocation(projection, event.reference);
    if (!invocation) return;
    invocation.end = index;
    invocation.endedAt = recordedAt;
    invocation.state = event.kind === 'execution_voided' ? 'skipped' : 'suspended';
    invocation.reason = event.reason;
    // The question this execution asked is recorded right after its deferral.
    if (event.kind === 'request_deferred') projection.asking = invocation;
  } else if (event.kind === 'safe_log') {
    const log = {
      index,
      timestamp: event.timestamp,
      stream: event.stream ?? 'output',
      text: event.line ?? '',
    };
    const invocation = event.execution == null ? undefined : byId.get(identity(event.execution));
    (invocation?.logs ?? systemLogs).push(log);
  } else if (event.kind === 'token_usage_observed' && event.execution != null) {
    const invocation = byId.get(identity(event.execution));
    if (invocation) invocation.usage = accumulateUsage(invocation.usage, event.usage);
  }
}

/** The still-open execution a settling event names, when its reference matches how it started. */
function openInvocation(
  projection: HistoryProjection,
  reference: Reference
): Invocation | undefined {
  const invocation = projection.byId.get(identity(reference.execution));
  return invocation &&
    sameReference(projection.references.get(invocation.id), reference, invocation.imported) &&
    invocation.end === undefined
    ? invocation
    : undefined;
}

/**
 * An earlier run's execution, carried into this one. One still active is held from that run (a
 * request awaiting its answer, or a wait), so it reads as waiting until it settles here.
 */
function readPriorExecution(prior: PriorExecution, index: number, projection: HistoryProjection) {
  const invocation = startInvocation(projection, index, {
    reference: {
      node: prior.occurrence.node,
      execution: prior.execution,
      nodeInstance: prior.node_instance,
    },
    mapIndices: prior.occurrence.mapIndices ?? [],
    attempt: prior.attempt ?? 1,
    input: prior.input,
  });
  if (!invocation) return;
  invocation.imported = true;
  if (prior.state === 'Active') invocation.state = 'suspended';
  else if ('Settled' in prior.state) {
    invocation.end = index;
    invocation.outcome = prior.state.Settled.outcome;
    invocation.state = prior.state.Settled.outcome.status === 'error' ? 'failed' : 'succeeded';
  } else if ('Voided' in prior.state) {
    invocation.end = index;
    invocation.state = 'skipped';
    invocation.reason = prior.state.Voided.reason;
  }
  // Inherited questions are recorded before the executions they came with.
  const question = projection.unowned.find((request) => couldHaveAsked(invocation, request));
  if (question) {
    invocation.request = question;
    projection.unowned.splice(projection.unowned.indexOf(question), 1);
  }
}

function startInvocation(
  projection: HistoryProjection,
  index: number,
  start: { reference: Reference; mapIndices: number[]; attempt: number; input: unknown }
): Invocation | undefined {
  const { invocations, byId, latestVisit, references } = projection;
  const id = identity(start.reference.execution);
  const instance = identity(start.reference.nodeInstance);
  if (byId.has(id)) return undefined;
  const node = start.reference.node;
  const scope = JSON.stringify([node, start.mapIndices]);
  const previous = latestVisit.get(scope);
  const retry =
    start.attempt > 1 && previous?.instance === instance && start.attempt > previous.attempt;
  const visit = retry ? previous.visit : (previous?.visit ?? 0) + 1;
  const invocation: Invocation = {
    id,
    node,
    instance,
    attempt: start.attempt,
    mapIndices: [...start.mapIndices],
    visit,
    start: index,
    input: start.input,
    state: 'running',
    logs: [],
  };
  latestVisit.set(scope, invocation);
  references.set(id, start.reference);
  byId.set(id, invocation);
  invocations.push(invocation);
  return invocation;
}

/** Requests, answers and expiry, each with the time it was recorded, and handoffs between runs. */
function readVector(vector: VectorRecord, index: number, projection: HistoryProjection) {
  const { requests, handoffs } = projection;
  const at = vector.createdAt;
  if (
    vector.kind === 'request' &&
    vector.node &&
    vector.text !== undefined &&
    vector.expiresAt !== undefined
  ) {
    recordRequest(projection, {
      id: vector.id,
      node: vector.node,
      text: vector.text,
      signals: vector.signals ?? {},
      output: vector.output,
      createdAt: at,
      expiresAt: vector.expiresAt,
      runId: vector.runId,
    });
  } else if (vector.kind === 'answer' || vector.kind === 'expiry') {
    const request = requests.find((known) => known.id === vector.requestId);
    if (!request) return;
    request.resolution =
      vector.kind === 'answer' && vector.answer
        ? { kind: 'answer', ...vector.answer, at, runId: vector.runId }
        : { kind: 'expiry', at, runId: vector.runId };
  } else if (vector.kind === 'continuation' && vector.resumedFrom) {
    handoffs.push({ kind: 'continued', index, from: vector.resumedFrom });
  }
}

/**
 * Keep one record per request and give it to the execution that asked. A question recorded in
 * this run follows its execution's deferral. One carried over from an earlier run has no deferral
 * here: it goes, in order, to the carried-over executions of its node that could have asked it,
 * whichever of the two the ledger records first.
 */
function recordRequest(projection: HistoryProjection, recorded: PersonRequest) {
  const known = projection.requests.find((request) => request.id === recorded.id);
  if (known) {
    Object.assign(known, recorded);
    return;
  }
  projection.requests.push(recorded);
  const asking = projection.asking;
  projection.asking = undefined;
  const owner =
    asking?.node === recorded.node && !asking.request
      ? asking
      : projection.invocations.find((invocation) => couldHaveAsked(invocation, recorded));
  if (owner) owner.request = recorded;
  else projection.unowned.push(recorded);
}

/** A carried-over execution of the request's node that has no question yet and was not voided
 * (voided work never reached its question; settled work, timed out or not, asked one). */
function couldHaveAsked(invocation: Invocation, request: PersonRequest): boolean {
  return (
    !!invocation.imported &&
    invocation.node === request.node &&
    !invocation.request &&
    invocation.state !== 'skipped'
  );
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

function sameReference(
  expected: Reference | undefined,
  actual: Reference,
  imported = false
): boolean {
  return (
    !!expected &&
    expected.node === actual.node &&
    identity(expected.execution) === identity(actual.execution) &&
    identity(expected.nodeInstance) === identity(actual.nodeInstance) &&
    // Imported executions name no run; the continuing run settles them under its own id.
    (expected.runId === actual.runId || (imported && expected.runId === undefined))
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

function controlObservation(visits: readonly ControlVisit[]): WorkflowNodeObservation {
  const latest = new Map(visits.map((visit) => [JSON.stringify(visit.mapIndices), visit]));
  const current = [...latest.values()];
  const states = new Set(current.map((visit) => controlState(visit.state)));
  const state =
    (['running', 'failed', 'skipped', 'recorded'] as const).find((next) => states.has(next)) ??
    'succeeded';
  const visit = current.reduce((a, b) => (a.updated > b.updated ? a : b));
  return {
    state,
    count: visits.length,
    countUnit: 'visit',
    detail: visit.branch ? `Selected: ${humanize(visit.branch)}` : visit.detail,
  };
}

function leafObservation(
  executions: readonly Invocation[],
  controls: readonly ControlVisit[],
  /** False once the run has ended: nothing waits on a question any more. */
  live = true
): WorkflowNodeObservation {
  const latest = new Map<string, { state: WorkflowNodeObservation['state']; start: number }>();
  for (const invocation of executions)
    latest.set(JSON.stringify(invocation.mapIndices), invocation);
  const queued = applyPendingVisits(latest, controls);
  const states = new Set([...latest.values()].map((visit) => visit.state));
  const current =
    (['running', 'suspended', 'failed', 'queued', 'skipped'] as const).find((next) =>
      states.has(next)
    ) ?? (latest.size ? 'succeeded' : 'idle');
  // Once the run has ended nothing waits any more: a step still held when it stopped was stopped.
  const state = !live && current === 'suspended' ? 'skipped' : current;
  const last = executions.at(-1);
  const signals = last?.outcome?.signals;
  // A pending question is what the step is waiting on; show it while the run still waits.
  const asking =
    live && last?.state === 'suspended' && last.request && !last.request.resolution
      ? `“${last.request.text}”`
      : undefined;
  return {
    state,
    count: executions.length,
    detail:
      [
        asking,
        signals &&
          Object.entries(signals)
            .map(([key, value]) => `${humanize(key)}: ${value}`)
            .join(' · '),
        queued ? `${queued} queued` : undefined,
      ]
        .filter(Boolean)
        .join(' · ') || undefined,
  };
}

function applyPendingVisits(
  latest: Map<string, { state: WorkflowNodeObservation['state']; start: number }>,
  controls: readonly ControlVisit[]
) {
  const current = new Map(controls.map((visit) => [JSON.stringify(visit.mapIndices), visit]));
  let queued = 0;
  for (const [key, visit] of current) {
    if (visit.state !== 'queued' && visit.state !== 'stopped') continue;
    const start = visit.queuedAt ?? visit.start;
    if ((latest.get(key)?.start ?? -1) >= start) continue;
    latest.set(key, { start, state: controlState(visit.state) });
    if (visit.state === 'queued') queued++;
  }
  return queued;
}
