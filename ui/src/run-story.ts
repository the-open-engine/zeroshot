import type { ControlVisit } from './control-history';
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
  /** Carried over from an earlier run; its transcript and recorded times stay with that run. */
  imported?: boolean;
};
type RoundEntry = { kind: 'round'; id: string; round: number; loop: string };
export type StoryEntry =
  | RoundEntry
  /** This run picked up where an earlier one paused. */
  | { kind: 'continued'; id: string; from: string }
  | {
      kind: 'work';
      id: string;
      state: NodeState;
      /** What kept several steps together: a parallel group or one pass over a map. */
      group?: string;
      steps: StoryStep[];
    };
export type StoryOutcome = { text: string; state: NodeState; facts: string[] };

type Execution = StoryStep & {
  start: number;
  /**
   * Innermost enclosing loop and its round, which places round dividers. `scope` is the loop's own
   * map position and outer rounds: each map item and each outer round counts its rounds afresh.
   */
  round?: { loop: string; number: number; scope: string };
  /**
   * Innermost par or map whose members run side by side, keyed by its map position and pass;
   * `lane` is the member's item of that map.
   */
  together?: { key: string; node: GraphNode; lane?: number };
};

export function projectStory(
  document: Document,
  invocations: readonly Invocation[],
  handoffs: readonly RunHandoff[] = [],
  /** The latest ledger time at the replay position, which bounds a running step. */
  at?: number,
  /** Structural visits, whose identities record each loop round. */
  controls: readonly ControlVisit[] = []
): StoryEntry[] {
  const executions = foldAttempts(document, invocations, controls, at);
  const entries: StoryEntry[] = [];
  let roundKey = '';
  let round: RoundEntry | undefined;
  let group: Execution[] = [];
  const placed: Placed = new Map();
  const flush = () => {
    if (!group.length) return;
    const steps = group.map(({ start, round, together, ...step }) => step);
    const container = group[0].together?.node;
    // Keyed by where the group began, so a live group keeps its rows (and open transcripts) as
    // members join or retry.
    const start = Math.min(...group.map((execution) => execution.start));
    const id = `work:${start}`;
    const items = new Set(group.map((execution) => execution.together?.lane)).size;
    placed.set(id, { start, round });
    entries.push({
      kind: 'work',
      id,
      state: worst(steps.map((step) => step.state)),
      group:
        steps.length > 1 && container
          ? container.kind === 'map'
            ? `${items} ${items === 1 ? 'item' : 'items'} · ${humanize(container.name)}`
            : `In parallel · ${humanize(container.name)}`
          : undefined,
      steps,
    });
    group = [];
  };
  for (const execution of executions) {
    const key = execution.round
      ? `${execution.round.loop}${execution.round.scope}#${execution.round.number}`
      : '';
    if (key !== roundKey) {
      flush();
      roundKey = key;
      round = execution.round && {
        kind: 'round',
        id: `round:${key}:${entries.length}`,
        round: execution.round.number,
        loop: humanize(execution.round.loop),
      };
      if (round) entries.push(round);
    }
    // One pass over a par or map stays together, however its lanes stagger the members' starts.
    const sideBySide =
      execution.together !== undefined && execution.together.key === group.at(-1)?.together?.key;
    if (!sideBySide) flush();
    group.push(execution);
  }
  flush();
  for (const handoff of handoffs) placeContinuation(entries, placed, handoff);
  return entries;
}

/** Each work entry's first start and the loop round it belongs to, which place handoffs. */
type Placed = Map<string, { start: number; round?: RoundEntry }>;

/**
 * Carried-over work keeps no timing, so a handoff is placed by what caused it: right after the
 * question its earlier run asked. Without one, it goes before the first work recorded after it.
 * A round can resume after a handoff, so work after it in the same round repeats its heading.
 */
function placeContinuation(entries: StoryEntry[], placed: Placed, handoff: RunHandoff) {
  const entry: StoryEntry = {
    kind: 'continued',
    id: `continued:${handoff.index}`,
    from: handoff.from,
  };
  const asked = entries.findIndex(
    (candidate) =>
      candidate.kind === 'work' &&
      candidate.steps.some((step) => step.request && step.request.runId === handoff.from)
  );
  const later = entries.findIndex(
    (candidate) =>
      candidate.kind === 'work' && (placed.get(candidate.id)?.start ?? -1) > handoff.index
  );
  // Without a question, a round heading directly before the later work stays with it.
  const at =
    asked >= 0
      ? asked + 1
      : later < 0
        ? entries.length
        : later > 0 && entries[later - 1].kind === 'round'
          ? later - 1
          : later;
  entries.splice(at, 0, entry);
  const next = entries[at + 1];
  const round = next?.kind === 'work' ? placed.get(next.id)?.round : undefined;
  if (round) entries.splice(at + 1, 0, { ...round, id: `${round.id}:after:${handoff.index}` });
}

function foldAttempts(
  document: Document,
  invocations: readonly Invocation[],
  controls: readonly ControlVisit[],
  at?: number
): Execution[] {
  return loopRounds(collectAttempts(document, invocations, controls, at));
}

/**
 * Without a recorded round, a node's visit count stands in, and it is the node's own: a step that
 * skips round 1 is still in round 2. A loop only moves forward, so each execution's round is at
 * least the round of the same loop's work recorded before it, in the same item and outer round.
 */
function loopRounds(executions: Execution[]): Execution[] {
  const current = new Map<string, number>();
  for (const execution of executions) {
    if (!execution.round) continue;
    const { loop, number, scope } = execution.round;
    const round = Math.max(number, current.get(loop + scope) ?? 1);
    current.set(loop + scope, round);
    execution.round = { ...execution.round, number: round };
  }
  return executions;
}

/**
 * The loop round an execution ran in, as recorded: every structural visit's identity carries the
 * rounds of its enclosing loops, so the nearest structural ancestor's latest visit before the
 * execution names them. A step that is the loop's whole body has none, and falls back to its visit.
 */
function recordedRound(
  ancestors: GraphNode[],
  loop: GraphNode,
  invocation: Invocation,
  controls: readonly ControlVisit[]
): Execution['round'] {
  const loops = ancestors.filter((node) => node.kind === 'loop');
  const position = loops.indexOf(loop);
  const fallback = {
    loop: loop.name,
    number: invocation.visit,
    scope: JSON.stringify(mapPrefix(ancestors, loop, invocation)),
  };
  const nearest = ancestors.at(-1);
  if (!nearest || ancestors.indexOf(nearest) <= ancestors.indexOf(loop)) return fallback;
  const prefix = JSON.stringify(mapPrefix(ancestors, nearest, invocation));
  const visit = controls
    .filter(
      (control) =>
        control.node === nearest.name &&
        JSON.stringify(control.mapIndices) === prefix &&
        control.start <= invocation.start
    )
    .reduce<ControlVisit | undefined>((a, b) => (a && a.start > b.start ? a : b), undefined);
  const rounds = visit && loopIterations(visit.visitId);
  if (!rounds || rounds.length !== loops.length) return fallback;
  return {
    loop: loop.name,
    number: rounds[position],
    scope: JSON.stringify([fallback.scope, rounds.slice(0, position)]),
  };
}

/** A structural visit's identity is `[node, mapIndices, loopIterations]`. */
function loopIterations(visitId: string): number[] | undefined {
  try {
    const rounds: unknown = JSON.parse(visitId)?.[2];
    return Array.isArray(rounds) && rounds.every((round) => Number.isInteger(round) && round > 0)
      ? rounds
      : undefined;
  } catch {
    return undefined;
  }
}

function collectAttempts(
  document: Document,
  invocations: readonly Invocation[],
  controls: readonly ControlVisit[],
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
        round: loop && recordedRound(ancestors, loop, latest, controls),
        together: concurrent && {
          // A member's own visit counts the passes over its group, as loop rounds re-enter it.
          key: JSON.stringify([
            concurrent.name,
            mapPrefix(ancestors, concurrent, latest),
            latest.visit,
          ]),
          node: concurrent,
          lane:
            concurrent.kind === 'map'
              ? latest.mapIndices[mapPrefix(ancestors, concurrent, latest).length]
              : undefined,
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

/**
 * The map positions above a group: items of one map run together; the same par inside different
 * map items does not.
 */
function mapPrefix(ancestors: GraphNode[], group: GraphNode, invocation: Invocation) {
  const depth = ancestors
    .slice(0, ancestors.indexOf(group))
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
  clock: { first?: number; at?: number } = {},
  /** The run an answer or resume continued this one in, which took over its open question. */
  successor?: string,
  controls: readonly ControlVisit[] = []
): StoryOutcome {
  const executions = foldAttempts(document, invocations, controls, clock.at);
  const rounds = new Map<string, number>();
  for (const { round } of executions)
    if (round) rounds.set(round.loop, Math.max(rounds.get(round.loop) ?? 0, round.number));
  const retried = executions.filter((execution) => execution.attempts > 1).length;
  const failed = executions.filter((execution) => execution.state === 'failed').length;
  const running = executions.filter((execution) => execution.state === 'running');
  const asking = executions.find(
    (execution) =>
      execution.state === 'suspended' && execution.request && !execution.request.resolution
  );
  // At the end of its history, a run that handed its question on no longer waits for it.
  const handedOff = !!asking?.request && !!successor && !open;
  const facts = [
    executions.length ? `${executions.length} ${executions.length === 1 ? 'step' : 'steps'}` : '',
    ...[...rounds].map(
      ([loop, count]) =>
        `${count} ${count === 1 ? 'round' : 'rounds'} of ${humanize(loop).toLowerCase()}`
    ),
    retried ? `${retried} retried` : '',
    failed ? `${failed} failed` : '',
    clock.first !== undefined && clock.at !== undefined && clock.at > clock.first
      ? `${formatDuration(clock.at - clock.first)}${terminal || handedOff ? '' : ' so far'}`
      : '',
    tokens(document, invocations),
  ].filter(Boolean);
  if (terminal)
    return terminal.status === 'succeeded'
      ? { text: 'Run completed', state: 'succeeded', facts }
      : { text: `Run failed · ${humanize(terminal.reason)}`, state: 'failed', facts };
  if (handedOff) return { text: `Continued in run ${successor}`, state: 'suspended', facts };
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
 * Steps that use no model (delivery, a person's answer) are never counted as missing, and
 * carried-over work is not counted at all: its usage stays with the run that did it.
 */
function tokens(document: Document, invocations: readonly Invocation[]) {
  const nodes = document.runtime.nodes as Record<string, { kind?: string } | undefined>;
  let total = 0,
    partial = false,
    known = false;
  for (const { node, usage, imported } of invocations) {
    if (imported) continue;
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
