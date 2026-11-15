import { useEffect, useMemo, useRef, type ReactNode } from 'react';
import { ChevronRight, CornerDownRight, UserRound } from 'lucide-react';
import type { FailureContext } from './failure-context';
import { humanize, type Invocation } from './run-history';
import { formatDuration, projectStory, type StoryEntry, type StoryStep } from './run-story';
import { formatMoment, requestStatus } from './PersonRequestView';
import {
  firstProse,
  hasValue,
  RunResultDetail,
  RunOutcomeBar,
  RunStartDetail,
  RunViewToolbar,
  StepDetail,
  SuccessorLink,
  useOpenSteps,
  type RunContext,
} from './RunViewParts';

type Target = { node: string; invocation?: string };

/**
 * Steps: the run in the order its work started, from what it was given to what it returned. Each
 * step reads as one line (a status dot, its name and decision, then what it said), and opens in
 * place for the rest of its record; so do the run's own start and end.
 */
export function RunStory({
  run,
  execution,
  select,
  showInGraph,
  toolbarStart,
  failureDetails,
  revealStart,
}: {
  run: RunContext;
  execution: string;
  select: (target: Target) => void;
  showInGraph: (target: Target) => void;
  toolbarStart: ReactNode;
  /** The runtime's recorded evidence for a failure, with its log links. */
  failureDetails: (context: FailureContext) => ReactNode;
  /** Changes when the run's log is asked for from elsewhere, which opens the run's start. */
  revealStart?: number;
}) {
  const entries = useMemo(
    () => projectStory(run.document, run.invocations, run.handoffs, run.clock.at),
    [run.document, run.invocations, run.handoffs, run.clock.at]
  );
  const byId = useMemo(
    () => new Map(run.invocations.map((invocation) => [invocation.id, invocation])),
    [run.invocations]
  );
  const sections = useMemo(() => frameSections(entries), [entries]);
  const steps = useOpenSteps(execution);
  const { terminal, currentFailure } = run;
  const failure = terminal?.status === 'failed' ? terminal : currentFailure;
  const startRef = useRef<HTMLLIElement>(null);
  const preparing = !run.invocations.length && run.systemLogs.length > 0;
  // Before any step runs, the run's log is all there is to read; a failure opens its evidence.
  useEffect(() => {
    if (preparing) steps.reveal(START);
  }, [preparing]);
  const failed = failure !== undefined;
  useEffect(() => {
    if (failed) steps.reveal(END);
  }, [failed]);
  useEffect(() => {
    if (!revealStart) return;
    steps.reveal(START);
    startRef.current?.scrollIntoView?.({ block: 'nearest' });
  }, [revealStart]);
  const start = (
    <li className="story-entry" ref={startRef}>
      <RunBoundary
        name="Started"
        meta={run.clock.first === undefined ? undefined : formatMoment(run.clock.first)}
        line={firstProse(run.input)}
        open={steps.isOpen(START)}
        toggle={() => steps.toggle(START)}
        detail={(hasValue(run.input) || run.systemLogs.length > 0) && <RunStartDetail run={run} />}
      />
    </li>
  );
  const end = (terminal || currentFailure) && (
    <li className="story-entry run-end">
      {failure ? (
        <RunBoundary
          state="failed"
          name="Failed"
          line={humanize(failure.reason)}
          open={steps.isOpen(END)}
          toggle={() => steps.toggle(END)}
          detail={
            (failure !== terminal || failure.context) && (
              <div className="step-detail">
                {failure !== terminal ? (
                  // Reported by the run, but past what the loaded history can replay.
                  <section aria-label="Current run status">
                    <p className="step-note">
                      The run failed. This status is separate from the available recorded playback.
                    </p>
                    {failure.context && failureDetails(failure.context)}
                  </section>
                ) : (
                  failureDetails(failure.context!)
                )}
              </div>
            )
          }
        />
      ) : (
        terminal?.status === 'succeeded' && (
          <RunBoundary
            state="succeeded"
            name="Completed"
            line={firstProse(terminal.output)}
            open={steps.isOpen(END)}
            toggle={() => steps.toggle(END)}
            detail={hasValue(terminal.output) && <RunResultDetail output={terminal.output} />}
          />
        )
      )}
    </li>
  );
  return (
    <section className="canvas-region run-steps-view" aria-label="Run steps">
      <RunViewToolbar toolbarStart={toolbarStart} />
      <div className="run-steps-scroll">
        <div className="run-steps-page">
          <RunOutcomeBar run={run} />
          <div className="story">
            {sections.map((section, index) => (
              <section key={section.id} className="story-section">
                {section.heading?.kind === 'round' && (
                  // The loop names the section; with several loops, its round alone is ambiguous.
                  <h3 className="story-round">
                    {section.heading.loop}
                    <span>Round {section.heading.round}</span>
                  </h3>
                )}
                {section.heading?.kind === 'continued' && (
                  <p className="story-handoff">
                    <CornerDownRight size={13} aria-hidden="true" />
                    <span>
                      Continued from{' '}
                      {run.runHref ? (
                        <a href={run.runHref(section.heading.from)}>{section.heading.from}</a>
                      ) : (
                        section.heading.from
                      )}
                    </span>
                  </p>
                )}
                <ol className="story-frame">
                  {index === 0 && start}
                  {section.work.map((entry) => (
                    <li key={entry.id} className="story-entry">
                      {entry.group && <p className="story-group">{entry.group}</p>}
                      {entry.steps.map((step) => (
                        <StoryLine
                          key={step.invocation}
                          step={step}
                          invocation={byId.get(step.invocation)}
                          run={run}
                          open={steps.isOpen(step.invocation)}
                          toggle={() => {
                            if (!steps.isOpen(step.invocation))
                              select({ node: step.node, invocation: step.invocation });
                            steps.toggle(step.invocation);
                          }}
                          showInGraph={() =>
                            showInGraph({ node: step.node, invocation: step.invocation })
                          }
                        />
                      ))}
                    </li>
                  ))}
                  {index === sections.length - 1 && end}
                </ol>
              </section>
            ))}
          </div>
          {!run.invocations.length && (
            <p className="run-steps-empty">No steps recorded at this point in the run.</p>
          )}
          <SuccessorLink run={run} />
        </div>
      </div>
    </section>
  );
}

/** The run's own start and end take keys no execution can have. */
const START = 'run:start';
const END = 'run:end';

/**
 * The run's own start or end on the rail, read and opened like a step: what it was given at the
 * top, what it returned or why it failed at the bottom. With nothing more to show, it does not open.
 */
function RunBoundary({
  state,
  name,
  meta,
  line,
  open,
  toggle,
  detail,
}: {
  state?: 'succeeded' | 'failed';
  name: string;
  meta?: string;
  line?: string;
  open: boolean;
  toggle: () => void;
  detail: ReactNode;
}) {
  const title = (
    <span className="story-title">
      <span className="story-name">{name}</span>
      {meta && <span className="story-meta">{meta}</span>}
      {detail && <ChevronRight className="story-chevron" size={14} aria-hidden="true" />}
    </span>
  );
  const text = !(open && detail) && line && (
    <span className="story-text" title={line}>
      {line}
    </span>
  );
  return (
    <div
      className={`story-step run-boundary state-${state ?? 'start'} ${open && detail ? 'open' : ''}`}
    >
      <span className="story-dot" aria-hidden="true" />
      {detail ? (
        <button className="story-step-head" aria-expanded={open} onClick={toggle}>
          {title}
          {text}
        </button>
      ) : (
        <div className="story-step-head">
          {title}
          {text}
        </div>
      )}
      {open && detail}
    </div>
  );
}

/** A round or a handoff titles a section; the steps after it share one frame. The first section
 * holds the run's start, so a list that opens with a round gets one without a title before it. */
function frameSections(entries: readonly StoryEntry[]): Section[] {
  const sections: Section[] = [{ id: 'start', work: [] }];
  for (const entry of entries) {
    if (entry.kind === 'work') sections.at(-1)!.work.push(entry);
    else sections.push({ id: entry.id, heading: entry, work: [] });
  }
  return sections;
}

type Work = Extract<StoryEntry, { kind: 'work' }>;
type Section = { id: string; heading?: Exclude<StoryEntry, Work>; work: Work[] };

function StoryLine({
  step,
  invocation,
  run,
  open,
  toggle,
  showInGraph,
}: {
  step: StoryStep;
  invocation?: Invocation;
  run: RunContext;
  open: boolean;
  toggle: () => void;
  showInGraph: () => void;
}) {
  const retried = step.attempts > 1;
  return (
    <div
      className={`story-step state-${step.state} ${retried ? 'retried' : ''} ${open ? 'open' : ''}`}
    >
      <span className="story-dot" aria-hidden="true" />
      <button className="story-step-head" aria-expanded={open} onClick={toggle}>
        <span className="story-title">
          {step.request && <UserRound size={14} aria-label="Asked a person" />}
          <span className="story-name">{step.label}</span>
          {step.decision && <span className="story-decision">{step.decision}</span>}
          {step.item && <span className="story-meta">{step.item}</span>}
          {retried && <span className="story-meta">{step.attempts} attempts</span>}
          {step.state === 'running' && <span className="story-meta">In progress</span>}
          <ChevronRight className="story-chevron" size={14} aria-hidden="true" />
          {step.durationMs !== undefined && (
            <span className="story-time" title="Recorded time from start to settlement">
              {formatDuration(step.durationMs)}
            </span>
          )}
        </span>
        {/* A request's question, or the step's latest output; open steps show both in full. */}
        {!open && step.request && (
          <>
            <span className="story-text">“{step.request.text}”</span>
            <span className="story-meta">{requestStatus(step.request)}</span>
          </>
        )}
        {!open && !step.request && step.line && (
          <span className="story-text" title={step.line}>
            {step.line}
          </span>
        )}
      </button>
      {open && invocation && (
        <StepDetail invocation={invocation} run={run} showInGraph={showInGraph} />
      )}
    </div>
  );
}
