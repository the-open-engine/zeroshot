import { pathTo, type Document, type GraphNode } from './domain';
import {
  executionDecision,
  humanize,
  type Invocation,
  type NodeState,
  type PersonRequest,
  type RunHandoff,
  type RunTerminal,
} from './run-history';

/**
 * The story view reads a run in recorded order: what each step did and decided, with steps that
 * ran side by side kept together and loop rounds marked as they begin. It never infers work that
 * history did not record.
 */
export type StoryStep = {
  node: string;
  label: string;
  /** Map item, when the step ran for one element of a map. */
  item?: string;
  invocation: string;
  state: NodeState;
  decision?: string;
  /** The step's latest recorded output line, which usually says what it did. */
  line?: string;
  attempts: number;
  /** The question a request node put to a person, and its answer or expiry. */
  request?: PersonRequest;
  /**
   * From the first attempt's start to the last one's settlement, or to the replay position while
   * it runs; unknown without ledger times, and for carried-over work, whose times are imports.
   */
  durationMs?: number;
  /** Carried over from an earlier run, which kept its output lines. */
  imported?: boolean;
};
export type StoryEntry =
  | { kind: 'round'; id: string; round: number; loop: string }
  /** This run picked up where an earlier one paused. */
  | { kind: 'continued'; id: string; from: string; at?: number }
  | {
      kind: 'work';
      id: string;
      state: NodeState;
      /** What kept several steps together: a parallel group or one pass over a map. */
      group?: string;
      /**
       * A step failed or needed another attempt. Signal values are never judged: graphs author
       * their own labels, and nothing records which of them is good or bad.
       */
      flagged: boolean;
      steps: StoryStep[];
    };
export type StoryOutcome = { text: string; state: NodeState; facts: string[] };

type Execution = StoryStep & {
  start: number;
  end?: number;
  /** Innermost enclosing loop and its round, which places round dividers. */
  round?: { loop: string; number: number };
  /** Innermost par or map whose members run side by side, keyed by its map position. */
  together?: { key: string; node: GraphNode };
};

export function projectStory(
  document: Document,
  invocations: readonly Invocation[],
  handoffs: readonly RunHandoff[] = [],
  /** The latest ledger time at the replay position, which bounds a running step. */
  at?: number
): StoryEntry[] {
  const executions = foldAttempts(document, invocations, at);
  const entries: StoryEntry[] = [];
  const continuations = handoffs.filter(
    (handoff): handoff is Extract<RunHandoff, { kind: 'continued' }> => handoff.kind === 'continued'
  );
  let roundKey = '';
  let group: Execution[] = [];
  const starts = new Map<string, number>();
  const flush = () => {
    if (!group.length) return;
    starts.set(
      group.map((execution) => execution.invocation).join('+'),
      Math.min(...group.map((execution) => execution.start))
    );
    const steps = group.map(({ start, end, round, together, ...step }) => step);
    const container = group[0].together?.node;
    entries.push({
      kind: 'work',
      id: group.map((execution) => execution.invocation).join('+'),
      state: worst(steps.map((step) => step.state)),
      group:
        steps.length > 1 && container
          ? container.kind === 'map'
            ? `${steps.length} items · ${humanize(container.name)}`
            : `In parallel · ${humanize(container.name)}`
          : undefined,
      flagged: steps.some((step) => step.state === 'failed' || step.attempts > 1),
      steps,
    });
    group = [];
  };
  for (const execution of executions) {
    const key = execution.round ? `${execution.round.loop}#${execution.round.number}` : '';
    if (key !== roundKey) {
      flush();
      roundKey = key;
      if (execution.round)
        entries.push({
          kind: 'round',
          // A round can resume after a handoff, so its heading may appear more than once.
          id: `round:${key}:${entries.length}`,
          round: execution.round.number,
          loop: humanize(execution.round.loop),
        });
    }
    const previous = group.at(-1);
    const sideBySide =
      previous &&
      execution.together !== undefined &&
      execution.together.key === previous.together?.key &&
      // Members of one group start before any of them settles; a later start is a new pass.
      // Carried-over work keeps no timing, so its members group by structure alone.
      (execution.imported && group.every((member) => member.imported)
        ? true
        : group.every((member) => member.end === undefined || member.end > execution.start));
    if (!sideBySide) flush();
    group.push(execution);
  }
  flush();
  for (const handoff of continuations) placeContinuation(entries, starts, handoff);
  return entries;
}

/**
 * Carried-over work keeps no timing, so a handoff is placed by what caused it: right after the
 * question its earlier run asked. Without one, it goes before the first work recorded after it.
 */
function placeContinuation(
  entries: StoryEntry[],
  starts: ReadonlyMap<string, number>,
  handoff: Extract<RunHandoff, { kind: 'continued' }>
) {
  const entry: StoryEntry = {
    kind: 'continued',
    id: `continued:${handoff.index}`,
    from: handoff.from,
    at: handoff.at,
  };
  const asked = entries.findIndex(
    (candidate) =>
      candidate.kind === 'work' &&
      candidate.steps.some((step) => step.request && step.request.runId === handoff.from)
  );
  if (asked >= 0) return void entries.splice(asked + 1, 0, entry);
  const later = entries.findIndex(
    (candidate) => candidate.kind === 'work' && (starts.get(candidate.id) ?? -1) > handoff.index
  );
  // A round heading directly before that work stays with it.
  const at = later > 0 && entries[later - 1].kind === 'round' ? later - 1 : later;
  entries.splice(at < 0 ? entries.length : at, 0, entry);
}

function foldAttempts(
  document: Document,
  invocations: readonly Invocation[],
  at?: number
): Execution[] {
  return loopRounds(collectAttempts(document, invocations, at));
}

/**
 * A node's visit count is its own, not the loop's: a step that skips round 1 is still in round 2.
 * A loop only moves forward, so each execution's round is at least the round of the loop work
 * recorded before it.
 */
function loopRounds(executions: Execution[]): Execution[] {
  const current = new Map<string, number>();
  for (const execution of executions) {
    if (!execution.round) continue;
    const { loop, number } = execution.round;
    const round = Math.max(number, current.get(loop) ?? 1);
    current.set(loop, round);
    execution.round = { loop, number: round };
  }
  return executions;
}

function collectAttempts(
  document: Document,
  invocations: readonly Invocation[],
  at?: number
): Execution[] {
  const executions = new Map<string, Invocation[]>();
  for (const invocation of invocations) {
    const key = JSON.stringify([invocation.node, invocation.visit, invocation.mapIndices]);
    executions.set(key, [...(executions.get(key) ?? []), invocation]);
  }
  return [...executions.values()]
    .map((attempts): Execution => {
      const latest = attempts.reduce((a, b) => (b.attempt >= a.attempt ? b : a));
      const ancestors = pathTo(document.graph.root, latest.node).slice(0, -1);
      const loop = [...ancestors].reverse().find((node) => node.kind === 'loop');
      const concurrent = [...ancestors]
        .reverse()
        .find((node) => node.kind === 'par' || node.kind === 'map');
      return {
        node: latest.node,
        label: humanize(latest.node),
        item: itemLabel(ancestors, latest.mapIndices),
        invocation: latest.id,
        state: latest.state,
        decision: executionDecision(latest),
        // A request's own words are its question, shown with the step rather than as output.
        line: latest.request ? undefined : latest.logs.at(-1)?.text,
        request: latest.request,
        imported: latest.imported,
        attempts: attempts.length,
        durationMs: duration(attempts, latest, at),
        start: Math.min(...attempts.map((invocation) => invocation.start)),
        end: latest.end,
        round: loop && { loop: loop.name, number: latest.visit },
        together: concurrent && {
          key: JSON.stringify([concurrent.name, mapPrefix(ancestors, concurrent, latest)]),
          node: concurrent,
        },
      };
    })
    .sort((a, b) => a.start - b.start);
}

function duration(attempts: readonly Invocation[], latest: Invocation, at?: number) {
  const started = attempts[0].startedAt;
  if (started === undefined || attempts.some((attempt) => attempt.imported)) return undefined;
  const ended = latest.endedAt ?? (latest.state === 'running' ? at : undefined);
  return ended === undefined ? undefined : Math.max(0, ended - started);
}

/** A compact duration for a step or a run: "<1s", "45s", "2m 05s", "1h 05m". */
export function formatDuration(ms: number) {
  const seconds = Math.round(ms / 1000);
  if (seconds < 1) return '<1s';
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, '0')}s`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, '0')}m`;
}

/** Items of one map run together; the same par inside different map items does not. */
function mapPrefix(ancestors: GraphNode[], concurrent: GraphNode, invocation: Invocation) {
  const depth = ancestors
    .slice(0, ancestors.indexOf(concurrent))
    .filter((node) => node.kind === 'map').length;
  return invocation.mapIndices.slice(0, depth);
}

function itemLabel(ancestors: GraphNode[], mapIndices: number[]) {
  if (!mapIndices.length) return undefined;
  const maps = ancestors.filter((node) => node.kind === 'map');
  return mapIndices
    .map((index, level) =>
      maps.length > 1
        ? `${humanize(maps[level]?.name ?? 'item')} ${index + 1}`
        : `Item ${index + 1}`
    )
    .join(' · ');
}

function worst(states: NodeState[]): NodeState {
  for (const state of ['running', 'failed', 'suspended', 'skipped'] as const)
    if (states.includes(state)) return state;
  return states.length ? 'succeeded' : 'idle';
}

/**
 * One line both list views open with: where the run stands at this position, then the facts
 * history recorded. `open` says whether unrecorded steps may still run here.
 */
export function projectOutcome(
  document: Document,
  invocations: readonly Invocation[],
  terminal: RunTerminal | undefined,
  open: boolean,
  /** The ledger's first and latest times at this position, when it kept times. */
  clock: { first?: number; at?: number } = {}
): StoryOutcome {
  const executions = foldAttempts(document, invocations, clock.at);
  const rounds = new Map<string, number>();
  for (const { round } of executions)
    if (round) rounds.set(round.loop, Math.max(rounds.get(round.loop) ?? 0, round.number));
  const retried = executions.filter((execution) => execution.attempts > 1).length;
  const failed = executions.filter((execution) => execution.state === 'failed').length;
  const running = executions.filter((execution) => execution.state === 'running');
  const facts = [
    executions.length ? `${executions.length} ${executions.length === 1 ? 'step' : 'steps'}` : '',
    ...[...rounds].map(
      ([loop, count]) =>
        `${count} ${count === 1 ? 'round' : 'rounds'} of ${humanize(loop).toLowerCase()}`
    ),
    retried ? `${retried} retried` : '',
    failed ? `${failed} failed` : '',
    clock.first !== undefined && clock.at !== undefined && clock.at > clock.first
      ? `${formatDuration(clock.at - clock.first)}${terminal ? '' : ' so far'}`
      : '',
    tokens(document, invocations),
  ].filter(Boolean);
  if (terminal)
    return terminal.status === 'succeeded'
      ? { text: 'Run completed', state: 'succeeded', facts }
      : { text: `Run failed · ${humanize(terminal.reason)}`, state: 'failed', facts };
  const asking = executions.find(
    (execution) =>
      execution.state === 'suspended' && execution.request && !execution.request.resolution
  );
  if (asking?.request)
    return { text: `Waiting for a person · ${asking.request.text}`, state: 'suspended', facts };
  if (running.length)
    return {
      text: `Running ${running
        .map((step) => (step.round ? `${step.label} (round ${step.round.number})` : step.label))
        .join(', ')}`,
      state: 'running',
      facts,
    };
  return open
    ? { text: 'Waiting for the next step', state: 'idle', facts }
    : { text: 'No further history recorded', state: 'idle', facts };
}

/**
 * Missing counters stay unknown: a total is a lower bound when an agent step reported none.
 * Steps that use no model (delivery, a person's answer) are never counted as missing.
 */
function tokens(document: Document, invocations: readonly Invocation[]) {
  const nodes = document.runtime.nodes as Record<string, { kind?: string } | undefined>;
  let total = 0,
    partial = false,
    known = false;
  for (const { node, usage } of invocations) {
    const input = usage?.inputTokens,
      output = usage?.outputTokens;
    if (typeof input === 'number' && typeof output === 'number') {
      known = true;
      total += input + output;
    } else if (nodes[node]?.kind === 'agent') partial = true;
  }
  if (!known) return '';
  const amount = new Intl.NumberFormat('en', {
    notation: 'compact',
    maximumFractionDigits: 1,
  }).format(total);
  return `${partial ? '≥ ' : ''}${amount} tokens`;
}
