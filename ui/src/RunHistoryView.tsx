import { useReplayDefinition } from './use-replay-definition';
import { useHistoryFeed } from './use-history-feed';
import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import { ChevronRight, Loader2 } from 'lucide-react';
import { InspectorPanel } from './InspectorPanel';
import { InspectorHeader } from './InspectorHeader';
import { NodeTrail } from './NodeTrail';
import { RunFailureDetails } from './RunFailureDetails';
import { RunTimeline } from './RunTimeline';
import {
  useRunView,
  historyViewCursor,
  restoredRunNode,
  useRunViewPersistence,
  type RunLayout,
  type RunViewState,
} from './run-view-state';
import { ExecutionTimeline } from './ExecutionTimeline';
import { ControlTimeline } from './ControlTimeline';
import { Transcript } from './Transcript';
import { InvocationView, ValueSection } from './InvocationDetail';
import { WorkflowCanvas } from './WorkflowCanvas';
import { RunStory } from './RunStory';
import type { RunContext } from './RunViewParts';
import { findNode, labels, type Document, type GraphNode, type Positions } from './domain';
import type { WorkerOption } from './workers';
import { nodeTitle } from './workflow-projection';
import type { WorkflowObservation } from './workflow-observation';
import { scopedExecutionId } from './history-contract';
import type { RunHistoryReader } from './run-history-source';
import {
  currentInvocation,
  descendantNames,
  historyMoments,
  nextReplayPosition,
  nodeReplayPositions,
  humanize,
  observedRoutes,
  projectHistory,
  suspensionStatus,
  type HistoryEvent,
  type Invocation,
  type RunDetail,
  type RunTerminal,
} from './run-history';
import { useProjection } from './use-run-projection';
import './run-history.css';

const dateLabel = (time?: number | null) =>
  time
    ? new Date(time).toLocaleString(undefined, {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      })
    : '';
type RunHistoryViewProps = {
  runId?: string;
  source: RunHistoryReader;
  listedPartId?: string;
  onOpenRun?: (run: RunDetail) => void;
  workers: WorkerOption[];
  emptyAction?: ReactNode;
  viewKey?: string;
  /** The host owns navigation between runs. */
  runHref?: (runId: string) => string;
  /** Set when a host page already shows the run's title and status above this view. */
  embedded?: boolean;
  /**
   * The successor the host's own run list reports for this run. A suspended run gains one after
   * this view read its detail; the host's existing refresh brings it without a reload.
   */
  successorRunId?: string;
};
/** What the graph shows of the run, and whether it follows a live run's latest events. */
function observed(
  key: string,
  nodes: WorkflowObservation['nodes'],
  following: boolean
): WorkflowObservation {
  return { key, nodes, following };
}

/** Where a node replay starts: its first event, unless the view is already part-way through it. */
function replayStart(replay: readonly number[], position: number, restart: boolean): number {
  const first = replay[0],
    last = replay.at(-1)!;
  return restart || position < first || position >= last ? first : position;
}

/** The selected recorded moment and whether it is also the loaded current position. */
function replayPosition(history: HistoryFeed, moments: ReturnType<typeof historyMoments>) {
  const { cursorIndex, events, loadedHead } = history;
  const safePosition = Math.max(
    0,
    moments.findIndex((moment) => moment.index === Math.min(cursorIndex, events.length - 1))
  );
  const moment = moments[safePosition];
  return { safePosition, moment, current: loadedHead && moment.index === events.length - 1 };
}

function useHistoryProjection(
  history: HistoryFeed,
  source: RunHistoryReader,
  listedSuccessor?: string
) {
  const { run, events, cursorIndex, control } = history;
  const moments = useMemo(() => historyMoments(events, cursorIndex), [events, cursorIndex]);
  const { safePosition, moment, current } = replayPosition(history, moments);
  const replayPart = events[moment.index]?.partId ?? events[0]?.partId;
  const definition = useReplayDefinition(run, replayPart, source);
  const { document } = definition;
  const projection = useProjection(
    document,
    events,
    moment.index,
    control,
    run,
    listedSuccessor,
    current
  );
  return { moments, safePosition, moment, current, definition, document, projection };
}

export function RunHistoryView({
  runId,
  source,
  workers,
  emptyAction,
  viewKey,
  runHref,
  embedded = false,
  successorRunId: listedSuccessor,
  onOpenRun,
  listedPartId,
}: RunHistoryViewProps) {
  const { saved: savedView, canvas, setCanvas, layout, setLayout } = useRunView(viewKey);
  const [playing, setPlaying] = useState(false);
  const failureView = useFailurePresentation(savedView);
  const [revealStart, setRevealStart] = useState(0);
  const [revealStep, setRevealStep] = useState<{ invocation: string; tick: number }>();
  const [playbackNode, setPlaybackNode] = useState<string>();
  const [selected, setSelected] = useState(''),
    [inspector, setInspector] = useState(false),
    [execution, setExecution] = useState('');
  const [readingExecution, setReadingExecution] = useState('');
  const [positions, setPositions] = useState<Positions>({}),
    [focus, setFocus] = useState<{ name: string; tick: number }>();
  const history = useHistoryFeed(
    runId,
    source,
    {
      restore: savedView,
      onLoad: () => {
        setPlaying(false);
        setPlaybackNode(undefined);
      },
      onReset: () => {
        failureView.reset();
        setCanvas(savedView.canvas);
        setInspector(Boolean(savedView.inspector));
        setExecution(savedView.execution ?? '');
        setReadingExecution('');
        setFocus(undefined);
        setPositions({});
      },
      onOpen: (detail) => {
        onOpenRun?.(detail);
        setSelected(restoredRunNode(detail.graph.root, savedView));
        setInspector(failureView.open(detail));
      },
      onMissing: () => setPlaying(false),
    },
    listedPartId
  );
  const { run, events, cursorIndex, setCursorIndex, following, loading, follow } = history;
  useRunViewPersistence(viewKey, run, loading, {
    selected,
    inspector,
    execution,
    following,
    canvas,
    layout,
    cursor: historyViewCursor(history.restoreCursor.current, events, cursorIndex),
  });
  const { moments, safePosition, moment, current, definition, document, projection } =
    useHistoryProjection(history, source, listedSuccessor);
  useEffect(() => {
    window.document.title = `${run?.title ?? 'Runs'} · Zeroshot`;
  }, [run?.title]);
  const clearExecution = () => {
    setExecution('');
    setReadingExecution('');
  };
  const replay = useReplayActions({
    history,
    source,
    failureView,
    document,
    moments,
    safePosition,
    selected,
    playing,
    setPlaying,
    playbackNode,
    setPlaybackNode,
    clearExecution,
  });
  function inspect(name: string, open = true) {
    setPlaying(false);
    setPlaybackNode(undefined);
    setSelected(name);
    clearExecution();
    if (open) setInspector(true);
  }
  function inspectDiagnostic(cursor: string) {
    const target = failureView.diagnostic(cursor, events, projection);
    if (!target || !document) return;
    setPlaying(false);
    follow(false);
    setReadingExecution('');
    setSelected(target.invocation?.node ?? document.graph.root.name);
    setExecution(target.invocation?.id ?? '');
    setInspector(true);
    setCursorIndex(target.index);
  }
  /** Steps has no inspector: a run-wide log opens the run's Started section instead. */
  function storyDiagnostic(cursor: string) {
    const target = failureView.diagnostic(cursor, events, projection);
    if (target && !target.invocation) setRevealStart(Date.now());
    else inspectDiagnostic(cursor);
  }
  function reveal(name: string) {
    inspect(name);
    setFocus({ name, tick: Date.now() });
  }
  function selectStep(target: { node: string; invocation?: string }) {
    setSelected(target.node);
    setExecution(target.invocation ?? '');
    setReadingExecution('');
  }
  function showStep(name: string) {
    const target = document && findNode(document.graph.root, name);
    const names = target && descendantNames(target);
    const latest = projection?.invocations.filter((item) => names?.has(item.node)).at(-1);
    if (!latest) return;
    selectStep({ node: name, invocation: latest.id });
    setRevealStep({ invocation: latest.id, tick: Date.now() });
  }
  const { terminal: displayedTerminal, currentFailure } = failureView.presentation(
    history,
    moment.index,
    projection?.terminal
  );
  const selection = useSelectedNode({
    document,
    projection,
    selected,
    execution,
    readingExecution,
    setReadingExecution,
    events,
    index: moment.index,
  });
  const { node } = selection;
  const hasLog = (cursor: string) => events.some((entry) => entry.cursor === cursor);
  /** A node picked in Graph stays picked in Steps: the execution its inspector shows opens there. */
  function switchLayout(view: RunLayout) {
    setLayout(view);
    const shown = selection.activeInvocation;
    if (view !== 'story' || !shown || !selected || selected === document?.graph.root.name) return;
    const id = shown.originalId ?? shown.id;
    setExecution(id);
    setRevealStep({ invocation: id, tick: Date.now() });
  }
  const toggle = <RunViewToggle layout={layout} setLayout={switchLayout} />;
  return (
    <main className="workspace history-workspace">
      {run && document && projection ? (
        <>
          <RunHistoryHeader
            run={run}
            terminal={displayedTerminal}
            history={history}
            position={safePosition}
            momentsCount={moments.length}
            openLog={() =>
              layout === 'story' ? setRevealStart(Date.now()) : inspect(document.graph.root.name)
            }
            retry={() => void history.loadHistory(run.runId, source, false)}
            embedded={embedded}
            runHref={runHref}
            successorRunId={projection.successorRunId}
          />
          <div
            className={`editor-body history-body ${inspector && layout === 'graph' ? '' : 'inspector-closed'}`}
          >
            {layout === 'story' ? (
              <RunStory
                run={runContext(run, document, projection, {
                  terminal: displayedTerminal,
                  currentFailure,
                  // Steps not recorded yet may still run when live or replaying before the end.
                  open:
                    !displayedTerminal &&
                    (history.isLiveRun || safePosition < moments.length - 1 || !history.loadedHead),
                  following,
                  answerable: current && run.phase === 'suspended',
                  runHref,
                })}
                execution={execution}
                select={selectStep}
                showInGraph={(target) => {
                  const work = projection.invocations.find((item) => item.id === target.invocation);
                  replay.inspectStep(work, moment.index);
                  selectStep(target);
                  setInspector(true);
                  setLayout('graph');
                  setFocus({ name: target.node, tick: Date.now() });
                }}
                toolbarStart={toggle}
                revealStart={revealStart}
                revealStep={revealStep}
                failureDetails={(context) => (
                  <RunFailureDetails context={context} openLog={storyDiagnostic} hasLog={hasLog} />
                )}
              />
            ) : (
              <ReactFlowProvider>
                <WorkflowCanvas
                  document={document}
                  selected={selected}
                  positions={positions}
                  setPositions={setPositions}
                  workers={workers}
                  select={inspect}
                  focus={focus}
                  observation={observed(
                    run.runId,
                    projection.nodes,
                    history.isLiveRun && following
                  )}
                  view={canvas}
                  onViewChange={setCanvas}
                  inspectorHidden={!inspector}
                  showInspector={() => setInspector(true)}
                  deselect={() => inspect(document.graph.root.name, false)}
                  toolbarStart={toggle}
                />
              </ReactFlowProvider>
            )}
            {inspector && node && layout === 'graph' && (
              <RunInspector
                run={run}
                document={document}
                projection={projection}
                terminal={displayedTerminal}
                currentFailure={currentFailure}
                moment={moment}
                {...selection}
                node={node}
                controlError={history.controlError}
                execution={execution}
                playingNode={playing && playbackNode === node.name}
                canReplay={!loading && replay.selectedReplay.length > 0}
                following={following}
                safePosition={safePosition}
                close={() => {
                  setInspector(false);
                  if (playbackNode) setPlaying(false);
                }}
                selectExecution={(id) => {
                  setPlaying(false);
                  follow(false);
                  setReadingExecution('');
                  setExecution(id);
                }}
                play={() => replay.playNode()}
                restart={() => replay.playNode(true)}
                reveal={reveal}
                inspectDiagnostic={inspectDiagnostic}
                hasLog={hasLog}
              />
            )}
          </div>
          <RunTimeline
            moment={moment}
            moments={moments}
            safePosition={safePosition}
            isLiveRun={history.isLiveRun}
            following={following}
            connection={history.connection}
            loading={loading}
            loadedHead={history.loadedHead}
            playing={playing}
            historyPending={history.historyPending}
            restorePending={history.restorePending}
            eventsCount={events.length}
            showNode={() => (layout === 'story' ? showStep : reveal)(moment.node!)}
            goLatest={replay.goLatest}
            scrub={replay.scrub}
            togglePlay={replay.togglePlay}
            loadMore={() => void history.loadHistory(run.runId, source, false)}
          />
        </>
      ) : (
        <HistoryEmpty
          historyPending={history.historyPending}
          loading={loading || (definition.loading && !definition.error)}
          error={definition.error ?? history.error}
          runId={runId}
          emptyAction={emptyAction}
          retry={() =>
            definition.error
              ? definition.retry()
              : runId && void history.loadHistory(runId, source, true)
          }
        />
      )}
    </main>
  );
}

/** Steps and Graph: one switch for the run's two views, leading the canvas toolbar. */
function RunViewToggle({
  layout,
  setLayout,
}: {
  layout: RunLayout;
  setLayout: (layout: RunLayout) => void;
}) {
  return (
    <div className="view-toggle" role="group" aria-label="Run view">
      {(['story', 'graph'] as const).map((view) => (
        <button
          key={view}
          type="button"
          aria-pressed={layout === view}
          onClick={() => setLayout(view)}
        >
          {view === 'graph' ? 'Graph' : 'Steps'}
        </button>
      ))}
    </div>
  );
}

/** What the Steps view reads about the run at this position. */
function runContext(
  run: RunDetail,
  document: Document,
  projection: NonNullable<ReturnType<typeof useProjection>>,
  position: Pick<
    RunContext,
    'terminal' | 'currentFailure' | 'open' | 'following' | 'answerable' | 'runHref'
  >
): RunContext {
  const successor = projection.successorRunId;
  return {
    ...position,
    document,
    runId: run.runId,
    input: run.initialInput,
    invocations: projection.invocations,
    handoffs: projection.handoffs,
    controls: projection.allControls,
    systemLogs: projection.systemLogs,
    clock: projection.clock,
    suspension: projection.suspension,
    now: projection.now,
    waiting:
      !position.terminal &&
      !successor &&
      (!!projection.suspension?.request ||
        projection.invocations.some(
          (invocation) =>
            invocation.state === 'suspended' && invocation.request && !invocation.request.resolution
        )),
    successor,
  };
}

/** The selected node's executions and controls, and which execution the inspector shows. */
function useSelectedNode({
  document,
  projection,
  selected,
  execution,
  readingExecution,
  setReadingExecution,
  events,
  index,
}: {
  document: Document | undefined;
  projection: ReturnType<typeof projectHistory> | undefined;
  selected: string;
  execution: string;
  readingExecution: string;
  setReadingExecution: (update: (previous: string) => string) => void;
  events: HistoryEvent[];
  index: number;
}) {
  const node = document && findNode(document.graph.root, selected);
  const names = node && descendantNames(node);
  const nodeInvocations =
    projection?.partInvocations.filter((invocation) => names?.has(invocation.node)) ?? [];
  const activeInvocation =
    nodeInvocations.find((invocation) => invocation.id === execution) ??
    nodeInvocations.find((invocation) => invocation.id === readingExecution) ??
    currentInvocation(nodeInvocations, events, index);
  const readingTranscript = useCallback(
    (reading: boolean) => {
      const id = activeInvocation?.id;
      if (id) setReadingExecution((previous) => (reading ? id : previous === id ? '' : previous));
    },
    [activeInvocation?.id]
  );
  return {
    node,
    nodeInvocations,
    activeInvocation,
    readingTranscript,
    nodeControls: projection?.controls.filter((visit) => visit.node === node?.name) ?? [],
    showExecutions:
      !!node &&
      (['step', 'verifier'].includes(node.kind) || node.name === document?.graph.root.name),
  };
}

/** Earlier Steps remain inspectable even if the head rewound past their executions. */
function graphPositionForStep(
  work: Invocation | undefined,
  events: readonly HistoryEvent[],
  through: number
) {
  if (!work?.partId || work.partId === events[through]?.partId) return;
  const last = Math.max(work.start, work.end ?? -1, work.logs.at(-1)?.index ?? -1);
  if (events[last]?.partId === work.partId) return last;
  // An imported wait or request can settle in a successor. Inspect its original part
  // through its own last record, including its suspension and transcript.
  for (let index = through; index >= work.start; index--)
    if (events[index].partId === work.partId) return index;
}

/** Seeking, playback and Go live: the timeline's controls, which pause or resume following. */
function useReplayActions({
  history,
  source,
  failureView,
  document,
  moments,
  safePosition,
  selected,
  playing,
  setPlaying,
  playbackNode,
  setPlaybackNode,
  clearExecution,
}: {
  history: HistoryFeed;
  source: RunHistoryReader;
  failureView: ReturnType<typeof useFailurePresentation>;
  document: Document | undefined;
  moments: ReturnType<typeof historyMoments>;
  safePosition: number;
  selected: string;
  playing: boolean;
  setPlaying: (playing: boolean) => void;
  playbackNode: string | undefined;
  setPlaybackNode: (node: string | undefined) => void;
  clearExecution: () => void;
}) {
  const { events, control, follow, setCursorIndex, run } = history;
  const selectedReplay = useMemo(
    () => (document ? nodeReplayPositions(document, moments, events, selected, control) : []),
    [document, moments, events, selected, control]
  );
  const replayScope = useMemo(
    () =>
      document && playbackNode
        ? nodeReplayPositions(document, moments, events, playbackNode, control)
        : undefined,
    [document, moments, events, playbackNode, control]
  );
  useReplayTimer({
    playing,
    setPlaying,
    position: safePosition,
    moments,
    scope: replayScope,
    cursorIndex: history.cursorIndex,
    playbackNode,
    setCursorIndex,
  });
  return {
    selectedReplay,
    inspectStep(work: Invocation | undefined, through: number) {
      const position = graphPositionForStep(work, events, through);
      if (position === undefined) return;
      setPlaying(false);
      setPlaybackNode(undefined);
      follow(false);
      setCursorIndex(position);
    },
    scrub(next: number) {
      failureView.replay();
      setPlaying(false);
      follow(false);
      clearExecution();
      setCursorIndex(moments[Math.max(0, Math.min(moments.length - 1, next))].index);
    },
    goLatest() {
      failureView.latest();
      setPlaying(false);
      setPlaybackNode(undefined);
      clearExecution();
      follow(history.isLiveRun);
      setCursorIndex(history.retained.current.length - 1);
      const { connection, loading, loadedHead } = history;
      if (run && (connection === 'limited' || connection === 'error' || (!loading && !loadedHead)))
        void history.loadHistory(run.runId, source, false);
    },
    playNode(restart = false) {
      if (!selectedReplay.length) return;
      failureView.replay();
      follow(false);
      if (!restart && playing && playbackNode === selected) {
        setPlaying(false);
        return;
      }
      const start = replayStart(selectedReplay, safePosition, restart);
      setCursorIndex(moments[start].index);
      clearExecution();
      setPlaybackNode(selected);
      setPlaying(nextReplayPosition(start, moments.length, selectedReplay) !== undefined);
    },
    togglePlay() {
      failureView.replay();
      follow(false);
      if (safePosition === moments.length - 1) setCursorIndex(-1);
      setPlaybackNode(undefined);
      clearExecution();
      setPlaying(!playing);
    },
  };
}

type HistoryFeed = ReturnType<typeof useHistoryFeed>;
function useFailurePresentation(saved: Partial<RunViewState>) {
  const [viewingLatest, setViewingLatest] = useState(true);
  return {
    reset() {
      setViewingLatest(!saved.cursor || saved.following === true);
    },
    open(detail: RunDetail) {
      setViewingLatest(!saved.cursor || saved.following === true || saved.cursor === detail.cursor);
      return (
        saved.inspector ??
        (detail.phase === 'admitted' ||
          !!environmentFailure(detail) ||
          (detail.terminal?.status === 'failed' && !!detail.terminal.context) ||
          !!detail.runtimeFailure?.context)
      );
    },
    replay() {
      setViewingLatest(false);
    },
    latest() {
      setViewingLatest(true);
    },
    presentation(history: HistoryFeed, index: number, observed: RunTerminal | undefined) {
      const atHead = index === history.events.length - 1;
      const failure = history.run?.runtimeFailure;
      const terminal =
        observed ??
        (history.loadedHead && atHead && failure
          ? { status: 'failed' as const, reason: failure.reason, context: failure.context }
          : undefined);
      return {
        terminal,
        currentFailure: terminal
          ? undefined
          : latestUnavailableFailure(history, atHead, viewingLatest),
      };
    },
    diagnostic(
      cursor: string,
      events: HistoryEvent[],
      projection: ReturnType<typeof projectHistory> | undefined
    ) {
      const index = events.findIndex((event) => event.cursor === cursor);
      if (index < 0) return;
      setViewingLatest(false);
      const event = events[index].event;
      const reference = event.reference ?? event.completion?.reference;
      const execution = event.execution ?? reference?.execution;
      const invocation = projection?.invocations.find(
        (item) =>
          execution != null && item.id === scopedExecutionId(events[index].partId, execution)
      );
      return { index, invocation };
    },
  };
}

function latestUnavailableFailure(history: HistoryFeed, atHead: boolean, viewingLatest: boolean) {
  if (!viewingLatest || history.restorePending || !atHead) return;
  if (history.loadedHead || history.loading || !history.error) return;
  return unavailableFailure(history.run);
}

function currentHistoryState(
  run: RunDetail,
  terminal: RunTerminal | undefined,
  preparing: boolean,
  atStart: boolean
) {
  if (run.phase === 'suspended') return suspensionStatus(run.suspension);
  if (run.phase === 'running' && run.suspendRequested) return 'Suspension requested';
  if (terminal) return terminal.status;
  if (run.runtimeFailure) return 'failed';
  if (preparing) return 'Preparing environment';
  if (atStart) return 'Before run';
  return run.phase === 'finished' ? 'Recorded history' : 'In progress';
}

/** Steps a playing replay one moment at a time and stops at the end of its scope. */
function useReplayTimer(replay: {
  playing: boolean;
  setPlaying: (playing: boolean) => void;
  position: number;
  moments: ReturnType<typeof historyMoments>;
  scope: ReturnType<typeof nodeReplayPositions> | undefined;
  cursorIndex: number;
  playbackNode: string | undefined;
  setCursorIndex: (index: number) => void;
}) {
  const { playing, setPlaying, position, moments, scope, cursorIndex, playbackNode } = replay;
  useEffect(() => {
    if (!playing) return;
    const next = nextReplayPosition(position, moments.length, scope);
    if (next === undefined) {
      setPlaying(false);
      return;
    }
    const timer = setTimeout(() => {
      replay.setCursorIndex(moments[next].index);
      if (nextReplayPosition(next, moments.length, scope) === undefined) setPlaying(false);
    }, 950);
    return () => clearTimeout(timer);
    // Each step owns its checkpoint until the timer fires; incoming live logs cannot postpone it.
  }, [playing, cursorIndex, playbackNode]);
}

function RunHistoryHeader({
  run,
  terminal,
  history,
  position,
  momentsCount,
  openLog,
  retry,
  embedded,
  runHref,
  successorRunId,
}: {
  run: RunDetail;
  terminal: RunTerminal | undefined;
  history: HistoryFeed;
  position: number;
  momentsCount: number;
  openLog: () => void;
  retry: () => void;
  embedded: boolean;
  runHref?: (runId: string) => string;
  successorRunId?: string;
}) {
  const preparing = isPreparingEnvironment(run, history.events);
  const shownStatus =
    history.loadedHead && position === momentsCount - 1
      ? currentHistoryState(run, terminal, preparing, position === 0)
      : 'Replay';
  return (
    <>
      {!embedded && (
        <div className="workspace-header history-header">
          <div className="profile-title">
            <h1 title={run.title}>{run.title}</h1>
            {!run.parts && successorRunId && runHref && (
              <a href={runHref(successorRunId)}>Continue to successor</a>
            )}
            <span
              className={`run-status state-${terminal?.status ?? (shownStatus === 'failed' ? 'failed' : 'replay')}`}
            >
              {run.example
                ? 'Simulated'
                : run.phase === 'suspended' || (run.phase === 'running' && run.suspendRequested)
                  ? shownStatus
                  : humanize(shownStatus)}
            </span>
          </div>
        </div>
      )}
      <RunHistoryAlerts
        run={run}
        error={history.error}
        loading={history.loading}
        preparing={preparing}
        openLog={openLog}
        retry={retry}
      />
    </>
  );
}

function unavailableFailure(run: RunDetail | undefined) {
  if (!['incomplete', 'expired', 'unavailable'].includes(run?.observation?.state ?? '')) return;
  if (run?.terminal?.status === 'failed') return run.terminal;
  const failure = run?.runtimeFailure;
  return failure && { status: 'failed' as const, reason: failure.reason, context: failure.context };
}

function isPreparingEnvironment(run: RunDetail | undefined, events: HistoryEvent[]): boolean {
  return (
    run?.phase === 'admitted' &&
    !events.some(({ event }) => event.kind === 'run_started' || event.kind === 'node_started')
  );
}

function environmentFailure(run: RunDetail): string | undefined {
  if (run.terminal?.status !== 'failed') return undefined;
  switch (run.terminal.reason) {
    case 'environment_setup_failed':
      return 'Environment setup failed.';
    case 'environment_startup_failed':
      return 'Environment startup failed.';
    case 'environment_preparation_timeout':
      return 'Environment preparation timed out.';
    default:
      return undefined;
  }
}

function EnvironmentNotice({
  run,
  preparing,
  openLog,
}: {
  run: RunDetail;
  preparing: boolean;
  openLog: () => void;
}) {
  const failure = environmentFailure(run);
  if (!preparing && !failure) return null;
  return (
    <div className="history-error" role={failure ? 'alert' : 'status'}>
      <span>
        {failure
          ? `${failure} Graph execution did not start. ` +
            'Check the run log and update the environment before starting a new run.'
          : 'Preparing environment. Setup and startup must finish before graph nodes can run.'}
      </span>
      <button className="text-button" onClick={openLog}>
        Open run log
      </button>
    </div>
  );
}

function RunHistoryAlerts({
  run,
  error,
  loading,
  preparing,
  openLog,
  retry,
}: {
  run: RunDetail;
  error: string;
  loading: boolean;
  preparing: boolean;
  openLog: () => void;
  retry: () => void;
}) {
  return (
    <>
      <EnvironmentNotice run={run} preparing={preparing} openLog={openLog} />
      {run.observation &&
        !(run.parts && run.observation.state === 'collecting') &&
        !['active', 'complete'].includes(run.observation.state) && (
          <div className="history-error" role="status" data-problem-code={run.observation.code}>
            <span>
              {run.observation.state === 'collecting'
                ? 'Collecting archived history. Playback will continue as records arrive.'
                : run.observation.state === 'expired'
                  ? 'This archive has expired.'
                  : run.observation.state === 'unavailable'
                    ? 'This run has no retained workspace history.'
                    : 'This archive is incomplete. The retained history remains available.'}
            </span>
          </div>
        )}
      {run.runtimeFailure && (
        <div className="history-error" role="alert">
          <span>History is incomplete: the runtime failed before its final state was saved.</span>
          <button className="text-button" disabled={loading} onClick={retry}>
            Reload
          </button>
        </div>
      )}
      {error && (
        <div className="history-error" role="alert">
          <span>{error}</span>
          <button className="text-button" onClick={retry}>
            Retry
          </button>
        </div>
      )}
    </>
  );
}

function HistoryEmpty({
  historyPending,
  loading,
  error,
  runId,
  emptyAction,
  retry,
}: {
  historyPending: boolean;
  loading: boolean;
  error: string;
  runId?: string;
  emptyAction?: ReactNode;
  retry: () => void;
}) {
  return (
    <div className="history-empty" role={historyPending ? 'status' : undefined}>
      {historyPending ? (
        <>
          <Loader2 size={23} className="spin" />
          <h1>Run view is still setting up</h1>
          <p>The graph will appear here automatically when it’s ready.</p>
        </>
      ) : loading ? (
        <>
          <Loader2 size={23} className="spin" />
          <h1>Loading run</h1>
        </>
      ) : error ? (
        <>
          <h1>Could not open this run</h1>
          <p>{error}</p>
          {runId && (
            <button className="button" onClick={retry}>
              Retry
            </button>
          )}
        </>
      ) : (
        <>
          <h1>Run history</h1>
          <p>Select a run to inspect its graph and recorded history.</p>
          {emptyAction}
        </>
      )}
    </div>
  );
}

type RunInspectorProps = {
  run: RunDetail;
  document: Document;
  projection: ReturnType<typeof projectHistory>;
  terminal?: RunTerminal;
  currentFailure?: Extract<RunTerminal, { status: 'failed' }>;
  node: GraphNode;
  moment: ReturnType<typeof historyMoments>[number];
  nodeControls: ReturnType<typeof projectHistory>['controls'];
  nodeInvocations: Invocation[];
  activeInvocation?: Invocation;
  showExecutions: boolean;
  controlError: string;
  execution: string;
  playingNode: boolean;
  canReplay: boolean;
  following: boolean;
  safePosition: number;
  close: () => void;
  selectExecution: (id: string) => void;
  play: () => void;
  restart: () => void;
  reveal: (name: string) => void;
  inspectDiagnostic: (cursor: string) => void;
  hasLog: (cursor: string) => boolean;
  readingTranscript: (reading: boolean) => void;
};

function RunInspector({
  run,
  document,
  projection,
  terminal,
  currentFailure,
  node,
  moment,
  nodeControls,
  nodeInvocations,
  activeInvocation,
  showExecutions,
  controlError,
  execution,
  playingNode,
  canReplay,
  following,
  safePosition,
  close,
  selectExecution,
  play,
  restart,
  reveal,
  inspectDiagnostic,
  hasLog,
  readingTranscript,
}: RunInspectorProps) {
  return (
    <InspectorPanel className="run-inspector" label="Run inspector">
      <InspectorHeader
        trail={
          <NodeTrail document={document} name={node.name} rootLabel="Run details" select={reveal} />
        }
        title={node.name === document.graph.root.name ? 'Run details' : nodeTitle(node.name)}
        kind={node.name === document.graph.root.name ? undefined : labels[node.kind]}
        close={close}
      />
      <div className="run-inspector-content">
        {node.name === document.graph.root.name && (
          <>
            <dl className="run-facts">
              <dt>Run</dt>
              <dd className="mono">{run.runId}</dd>
              {run.createdAt && (
                <>
                  <dt>Started</dt>
                  <dd>{dateLabel(run.createdAt)}</dd>
                </>
              )}
              <dt>Runtime</dt>
              <dd>
                {document.runtime.harness} / {document.runtime.provider}
              </dd>
              <dt>At cursor</dt>
              <dd className="mono">{moment.cursor}</dd>
            </dl>
            {currentFailure && (
              <section aria-label="Current run status">
                <h3>Current run status</h3>
                <p className="history-muted">
                  The run failed. This status is separate from the available recorded playback.
                </p>
                <ValueSection title="Failure reason" value={currentFailure.reason} open />
                {currentFailure.context && (
                  <RunFailureDetails
                    context={currentFailure.context}
                    openLog={inspectDiagnostic}
                    hasLog={hasLog}
                  />
                )}
              </section>
            )}
            {terminal && (
              <ValueSection
                title={terminal.status === 'succeeded' ? 'Run output' : 'Failure reason'}
                value={terminal.status === 'succeeded' ? terminal.output : terminal.reason}
                open
              />
            )}
            {terminal?.status === 'failed' && terminal.context && (
              <RunFailureDetails
                context={terminal.context}
                openLog={inspectDiagnostic}
                hasLog={hasLog}
              />
            )}
            <ValueSection title="Run inputs" value={run.initialInput} />
            <details className="history-value">
              <summary>Recording details</summary>
              <p className="history-muted">
                Worker executions and transcripts come from the ledger. Control visits are
                reconstructed by the graph runtime at each recorded checkpoint.
              </p>
              <ValueSection title="Source" value={run.source} />
            </details>
          </>
        )}
        {node.kind === 'profile' && (
          <dl className="run-facts">
            <dt>Saved profile</dt>
            <dd className="mono">{node.profile}</dd>
          </dl>
        )}
        {!nodeControls.length && observedRoutes(node, projection.partInvocations).length > 0 && (
          <section className="run-inspector-section">
            <h3>Observed paths</h3>
            {observedRoutes(node, projection.partInvocations).map((route) => (
              <button
                className="observed-route"
                key={route.name}
                onClick={() => reveal(route.name)}
              >
                {humanize(route.name)}
                <span>{route.count} executions</span>
                <ChevronRight size={14} />
              </button>
            ))}
          </section>
        )}
        {!showExecutions && (
          <>
            <ControlTimeline
              visits={nodeControls}
              selected={execution}
              select={selectExecution}
              reveal={reveal}
              playing={playingNode}
              canReplay={canReplay}
              play={play}
              restart={restart}
            />
            {controlError ? (
              <p className="history-muted" role="status">
                {controlError}
              </p>
            ) : (
              !nodeControls.length && (
                <p className="history-muted">
                  {run.example
                    ? 'Control visits are not included in this example recording.'
                    : 'No visits at this point.'}
                </p>
              )
            )}
          </>
        )}
        {showExecutions && (
          <>
            <ExecutionTimeline
              invocations={nodeInvocations}
              selected={activeInvocation?.id}
              node={node.name}
              document={document}
              select={selectExecution}
              playing={playingNode}
              canReplay={canReplay}
              play={play}
              restart={restart}
            />
            {nodeInvocations.length > 0 ? (
              <>
                {activeInvocation && (
                  <InvocationView
                    key={activeInvocation.id}
                    invocation={activeInvocation}
                    reveal={() => reveal(activeInvocation.node)}
                    showNode={activeInvocation.node !== node.name}
                    follow={following}
                    reading={readingTranscript}
                  />
                )}
              </>
            ) : (
              <p className="history-muted">
                {safePosition === 0
                  ? 'The run has not started at this point.'
                  : 'No executions recorded here at this point.'}
              </p>
            )}
          </>
        )}
        {node.name === document.graph.root.name && projection.systemLogs.length > 0 && (
          <Transcript title="Run log" logs={projection.systemLogs} follow={following} />
        )}
        <ValueSection title="Definition" value={node} raw />
        <ValueSection
          title="Runtime configuration"
          value={
            node.name === document.graph.root.name
              ? document.runtime
              : document.runtime.nodes[node.name]
          }
          raw
        />
      </div>
    </InspectorPanel>
  );
}
