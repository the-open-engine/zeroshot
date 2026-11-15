import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { ChevronRight } from 'lucide-react';
import type { ControlVisit } from './control-history';
import type { Document } from './domain';
import { PersonRequestView } from './PersonRequestView';
import { RecordedValue } from './RecordedValue';
import { Transcript } from './Transcript';
import { humanize, type Invocation, type RunHandoff, type RunTerminal } from './run-history';
import { projectOutcome } from './run-story';
import './run-story.css';

/** What the Steps view needs about the run itself, beyond its executions. */
export type RunContext = {
  document: Document;
  runId: string;
  input: unknown;
  invocations: readonly Invocation[];
  handoffs: readonly RunHandoff[];
  /** Structural visits, whose identities record loop rounds. */
  controls: readonly ControlVisit[];
  systemLogs: Invocation['logs'];
  /** The ledger's first and latest recorded times at this position, when it kept times. */
  clock: { first?: number; at?: number };
  terminal?: RunTerminal;
  /** A failure the run reported that the loaded history does not reach yet. */
  currentFailure?: Extract<RunTerminal, { status: 'failed' }>;
  /** Unrecorded steps may still run at this position: live, or replaying before the end. */
  open: boolean;
  /** The view follows the latest event, so new output streams into open transcripts. */
  following: boolean;
  /** The run is suspended waiting for a person now; a pending question can still be answered. */
  waiting: boolean;
  /** The run that took over this one, once the replay reaches the end of this run's history. */
  successor?: string;
  runHref?: (id: string) => string;
};

/** The canvas toolbar for the Steps view. Details open in place there, so it has no inspector. */
export function RunViewToolbar({ toolbarStart }: { toolbarStart: ReactNode }) {
  return (
    <div className="canvas-toolbar">
      <div className="canvas-start">{toolbarStart}</div>
    </div>
  );
}

/** Where the run stands at the replay position, then what history recorded so far. */
export function RunOutcomeBar({ run }: { run: RunContext }) {
  const { document, invocations, terminal, open, clock, successor, controls } = run;
  const outcome = useMemo(
    () => projectOutcome(document, invocations, terminal, open, clock, successor, controls),
    [document, invocations, terminal, open, clock, successor, controls]
  );
  // Only the run's state is announced, and only while following it live: replay and its
  // running counts would otherwise re-announce on every step.
  return (
    <div className={`run-outcome state-${outcome.state}`}>
      <span className="run-outcome-text" aria-live={run.following ? 'polite' : 'off'}>
        {outcome.text}
      </span>
      {outcome.facts.length > 0 && (
        <span className="run-outcome-facts">{outcome.facts.join(' · ')}</span>
      )}
    </div>
  );
}

/** What the run was given and what its runtime said outside any step, opened at the list's top. */
export function RunStartDetail({ run }: { run: RunContext }) {
  if (!hasValue(run.input) && !run.systemLogs.length) return null;
  return (
    <div className="step-detail">
      {hasValue(run.input) && (
        <DetailSection title="Input" hint={fieldNames(run.input)}>
          <Facts value={run.input} />
        </DetailSection>
      )}
      {run.systemLogs.length > 0 && (
        <DetailSection title="Run log" count={lines(run.systemLogs.length)}>
          <Transcript title="Run log" logs={run.systemLogs} follow={run.following} titled={false} />
        </DetailSection>
      )}
    </div>
  );
}

/** What the run returned, opened at the list's end. */
export function RunResultDetail({ output }: { output: unknown }) {
  return (
    <div className="step-detail">
      <DetailSection title="Result" hint={fieldNames(output)}>
        <Facts value={output} />
      </DetailSection>
    </div>
  );
}

/** A record's first sentence-like text, which usually says what it is about. */
export function firstProse(value: unknown): string | undefined {
  if (typeof value === 'string') return value.trim() || undefined;
  if (value && typeof value === 'object' && !Array.isArray(value))
    return Object.values(value).find(
      (field): field is string => typeof field === 'string' && /\s/.test(field.trim())
    );
  return undefined;
}

/**
 * One step's record, opened under it, in the order it happened: what the step was given (Input, or
 * a person's question), what it printed while working (Transcript), what it returned (Result), and
 * its own explanation when that adds anything (Notes). Each section is a title on the paper over
 * framed rows, as on Platform's item pages; its state and decision are already on its line.
 */
export function StepDetail({
  invocation,
  run,
  showInGraph,
}: {
  invocation: Invocation;
  run: RunContext;
  showInGraph: () => void;
}) {
  const outcome = invocation.outcome;
  const failure = outcome?.status === 'error' ? outcome : undefined;
  const said = new Set(invocation.logs.map((log) => log.text.trim()));
  const notes = hasValue(outcome?.diagnostic) && !repeats(outcome!.diagnostic, said);
  return (
    <div className="step-detail">
      {invocation.request ? (
        <PersonRequestView
          request={invocation.request}
          runId={invocation.request.runId ?? run.runId}
          context={run}
          runHref={run.runHref}
        />
      ) : (
        hasValue(invocation.input) && (
          <DetailSection title="Input" hint={fieldNames(invocation.input)}>
            <Facts value={invocation.input} />
          </DetailSection>
        )
      )}
      {invocation.logs.length > 0 ? (
        <DetailSection title="Transcript" count={lines(invocation.logs.length)}>
          <Transcript logs={invocation.logs} follow={run.following} titled={false} />
        </DetailSection>
      ) : (
        // A person's answer has no transcript anywhere; only an agent's stays with its run.
        invocation.imported &&
        !invocation.request && (
          <p className="step-note">
            Carried over from an earlier run; its transcript stays with that run.
          </p>
        )
      )}
      {/* The failure's code is already the step's decision; only a further reason is new. */}
      {failure?.reason && failure.reason !== 'declared_failure' && (
        <p className="step-failure">{humanize(failure.reason)}</p>
      )}
      {invocation.reason && <p className="step-note">Stopped: {humanize(invocation.reason)}</p>}
      {hasValue(outcome?.output) && (
        <DetailSection
          title={invocation.request ? 'Answer' : 'Result'}
          hint={fieldNames(outcome!.output)}
        >
          <Facts value={outcome!.output} />
        </DetailSection>
      )}
      {notes && (
        <DetailSection title="Notes" hint={fieldNames(outcome!.diagnostic)}>
          <Facts value={outcome!.diagnostic} />
        </DetailSection>
      )}
      {invocation.usage && Object.keys(invocation.usage).length > 0 && (
        <DetailSection title="Usage" hint={tokenTotal(invocation.usage)}>
          <Facts value={tokenFacts(invocation.usage)} />
        </DetailSection>
      )}
      {!!outcome?.artifacts?.length && (
        <DetailSection title="Artifacts" count={lines(outcome.artifacts.length, 'artifact')}>
          <RecordedValue value={outcome.artifacts} />
        </DetailSection>
      )}
      <button className="step-link" onClick={showInGraph}>
        Show in graph
      </button>
    </div>
  );
}

/** Every string a diagnostic holds already appears in what the step printed. */
function repeats(value: unknown, said: ReadonlySet<string>): boolean {
  if (typeof value === 'string') return said.has(value.trim());
  if (value && typeof value === 'object')
    return Object.values(value).every((field) => repeats(field, said));
  return false;
}

/**
 * A titled section on the paper. Every section starts open and folds the same way; folded, its
 * title is followed by a muted hint of what is inside, like Platform's "Technical details".
 */
function DetailSection({
  title,
  hint,
  count,
  children,
}: {
  title: string;
  /** What is inside, shown only while the section is folded. */
  hint?: string;
  /** How much is inside, shown always. */
  count?: string;
  children: ReactNode;
}) {
  return (
    <details className="step-section" open>
      <summary>
        <ChevronRight size={14} aria-hidden="true" />
        {title}
        {count && <small>{count}</small>}
        {hint && <small className="step-section-hint">{hint}</small>}
      </summary>
      <div className="step-section-body">{children}</div>
    </details>
  );
}

const compactCount = new Intl.NumberFormat(undefined, {
  notation: 'compact',
  maximumFractionDigits: 1,
});
const exactCount = new Intl.NumberFormat();

/** Provider-reported token counters, named as people read them, each exact. */
function tokenFacts(usage: Record<string, number>) {
  const names: Record<string, string> = {
    inputTokens: 'Input',
    outputTokens: 'Output',
    cacheReadInputTokens: 'Cached input read',
    cacheCreationInputTokens: 'Cached input written',
  };
  return Object.fromEntries(
    Object.entries(usage).map(([key, count]) => [names[key] ?? key, exactCount.format(count)])
  );
}

function tokenTotal(usage: Record<string, number>) {
  const { inputTokens, outputTokens } = usage;
  return typeof inputTokens === 'number' && typeof outputTokens === 'number'
    ? `${compactCount.format(inputTokens + outputTokens)} tokens`
    : undefined;
}

export const fieldNames = (value: unknown) =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? Object.keys(value).map(sentence).join(', ')
    : undefined;
const lines = (count: number, unit = 'line') => `${count} ${unit}${count === 1 ? '' : 's'}`;

/** Long text keeps the recorded value's paging rather than rendering every character at once. */
const INLINE_TEXT = 600;

/** Values that read as identifiers (no spaces, short) are code; prose stays in the UI face. */
const identifier = (value: string) => value.length <= 80 && !/\s/.test(value);

/** A flat record as framed rows: a name column beside each value (Platform's framed rows). */
function Facts({ value }: { value: unknown }) {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    return (
      <div className="step-frame">
        <div className="step-row step-row-wide">
          <RecordedValue value={value} />
        </div>
      </div>
    );
  return (
    <dl className="step-frame">
      {Object.entries(value).map(([key, field]) => (
        <div className="step-row" key={key}>
          <dt>{sentence(key)}</dt>
          <dd>
            {typeof field === 'string' && field.length <= INLINE_TEXT ? (
              !field ? (
                <span className="step-empty">Empty</span>
              ) : identifier(field) ? (
                <code title={field}>{field}</code>
              ) : (
                <span className="step-prose">{field}</span>
              )
            ) : typeof field === 'number' || typeof field === 'boolean' ? (
              <code>{String(field)}</code>
            ) : field === null ? (
              <span className="step-empty">None</span>
            ) : (
              <RecordedValue value={field} />
            )}
          </dd>
        </div>
      ))}
    </dl>
  );
}

/** Rows opened by the reader, plus whichever execution is selected from elsewhere. */
export function useOpenSteps(execution: string) {
  const [open, setOpen] = useState<Set<string>>(new Set());
  useEffect(() => {
    if (execution) setOpen((previous) => new Set(previous).add(execution));
  }, [execution]);
  const toggle = (id: string) =>
    setOpen((previous) => {
      const next = new Set(previous);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const reveal = (id: string) => setOpen((previous) => new Set(previous).add(id));
  return { isOpen: (id: string) => open.has(id), toggle, reveal };
}

/** The next run an answer or resume started, linked at the end of the list. */
export function SuccessorLink({ run }: { run: RunContext }) {
  if (!run.successor) return null;
  const label = `Continued in run ${run.successor}`;
  return run.runHref ? (
    <a className="run-handoff" href={run.runHref(run.successor)}>
      {label}
      <ChevronRight size={14} />
    </a>
  ) : (
    <p className="run-handoff">{label}</p>
  );
}

/** Field names in sentence case (ui/DESIGN.md): "headRevision" reads "Head revision". */
function sentence(key: string) {
  const words = humanize(key);
  return words.charAt(0) + words.slice(1).toLowerCase();
}

export function hasValue(value: unknown) {
  if (value == null) return false;
  if (typeof value === 'object') return Object.keys(value).length > 0;
  return value !== '';
}
