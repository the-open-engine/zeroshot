import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
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
  restoredHistoryPosition,
  useRunViewPersistence,
  type RunViewState,
} from './run-view-state';
import { ExecutionTimeline } from './ExecutionTimeline';
import { ControlTimeline } from './ControlTimeline';
import { appendControlHistory, type ControlRecord } from './control-history';
import { RecordedValue } from './RecordedValue';
import { Transcript } from './Transcript';
import { WorkflowCanvas } from './WorkflowCanvas';
import { findNode, labels, type Document, type GraphNode, type Positions } from './domain';
import type { WorkerOption } from './workers';
import { nodeTitle } from './workflow-projection';
import type { WorkflowObservation } from './workflow-observation';
import { appendHistory, type RunHistoryReader } from './run-history-source';
import {
  currentInvocation,
  descendantNames,
  finishRunHistory,
  historyMoments,
  nextReplayPosition,
  nodeReplayPositions,
  humanize,
  observedRoutes,
  projectHistory,
  requestStanding,
  stateLabel,
  suspensionStatus,
  type HistoryEvent,
  type HistoryPage,
  type Invocation,
  type RunDetail,
  type RunTerminal,
} from './run-history';
import { canFollowHistory, observationEnded } from './history-contract';
import { readHistoryWhenReady } from './history-readiness';
import './run-history.css';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
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

export function RunHistoryView({
  runId,
  source,
  workers,
  emptyAction,
  viewKey,
  runHref,
  embedded = false,
  successorRunId: listedSuccessor,
}: RunHistoryViewProps) {
  const { saved: savedView, canvas, setCanvas } = useRunView(viewKey);
  const [playing, setPlaying] = useState(false);
  const failureView = useFailurePresentation(savedView);
  const [playbackNode, setPlaybackNode] = useState<string>();
  const [selected, setSelected] = useState(''),
    [inspector, setInspector] = useState(false),
    [execution, setExecution] = useState('');
  const [readingExecution, setReadingExecution] = useState('');
  const [positions, setPositions] = useState<Positions>({}),
    [focus, setFocus] = useState<{ name: string; tick: number }>();
  const history = useHistoryFeed(runId, source, {
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
      setSelected(restoredRunNode(detail.graph.root, savedView));
      setInspector(failureView.open(detail));
    },
    onMissing: () => setPlaying(false),
  });
  const {
    run,
    events,
    loadedHead,
    control,
    controlError,
    cursorIndex,
    setCursorIndex,
    following,
    connection,
    loading,
    historyPending,
    error,
    retained,
    restoreCursor,
    restorePending,
    isLiveRun,
    follow,
    loadHistory,
  } = history;
  useRunViewPersistence(viewKey, run, loading, {
    selected,
    inspector,
    execution,
    following,
    canvas,
    cursor: historyViewCursor(restoreCursor.current, events, cursorIndex),
  });
  const document: Document | undefined = useMemo(
    () => run && { name: run.runId, graph: run.graph, runtime: run.runtime },
    [run]
  );
  const moments = useMemo(() => historyMoments(events, cursorIndex), [events, cursorIndex]);
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
  const safePosition = Math.max(
      0,
      moments.findIndex((moment) => moment.index === Math.min(cursorIndex, events.length - 1))
    ),
    moment = moments[safePosition];
  const projection = useMemo(
    () => document && projectHistory(document, events, moment.index, control),
    [document, events, moment.index, control]
  );
  const successorRunId = run?.workspaceRecovery?.successorRunId ?? listedSuccessor;
  const now = useDeadlineClock(
    run?.phase === 'suspended' && !successorRunId ? run.suspension?.request?.expiresAt : undefined
  );
  const observedNodes = useMemo(
    () =>
      projection &&
      requestStanding(
        projection.nodes,
        run?.phase === 'suspended' ? run.suspension : undefined,
        !!successorRunId,
        now
      ),
    [projection, run, successorRunId, now]
  );
  useEffect(() => {
    window.document.title = `${run?.title ?? 'Runs'} · Zeroshot`;
  }, [run?.title]);

  useReplayTimer({
    playing,
    setPlaying,
    position: safePosition,
    moments,
    scope: replayScope,
    cursorIndex,
    playbackNode,
    setCursorIndex,
  });
  function scrub(next: number) {
    failureView.replay();
    setPlaying(false);
    follow(false);
    setExecution('');
    setReadingExecution('');
    setCursorIndex(moments[Math.max(0, Math.min(moments.length - 1, next))].index);
  }
  function goLatest() {
    failureView.latest();
    setPlaying(false);
    setPlaybackNode(undefined);
    setExecution('');
    setReadingExecution('');
    follow(isLiveRun);
    setCursorIndex(retained.current.length - 1);
    if (run && (connection === 'limited' || connection === 'error' || (!loading && !loadedHead)))
      void loadHistory(run.runId, source, false);
  }
  /** Select a node, stopping any node replay; the empty canvas selects without opening the panel. */
  function inspect(name: string, open = true) {
    setPlaying(false);
    setPlaybackNode(undefined);
    setSelected(name);
    setExecution('');
    setReadingExecution('');
    if (open) setInspector(true);
  }
  function playNode(restart = false) {
    if (!selectedReplay.length) return;
    failureView.replay();
    follow(false);
    if (!restart && playing && playbackNode === selected) {
      setPlaying(false);
      return;
    }
    const start = replayStart(selectedReplay, safePosition, restart);
    setCursorIndex(moments[start].index);
    setExecution('');
    setReadingExecution('');
    setPlaybackNode(selected);
    setPlaying(nextReplayPosition(start, moments.length, selectedReplay) !== undefined);
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
  function reveal(name: string) {
    inspect(name);
    setFocus({ name, tick: Date.now() });
  }
  const { terminal: displayedTerminal, currentFailure } = failureView.presentation(
    history,
    moment.index,
    projection?.terminal
  );
  const node = document && findNode(document.graph.root, selected);
  const names = node && descendantNames(node);
  const nodeInvocations =
    projection?.invocations.filter((invocation) => names?.has(invocation.node)) ?? [];
  const activeInvocation =
    nodeInvocations.find((invocation) => invocation.id === execution) ??
    nodeInvocations.find((invocation) => invocation.id === readingExecution) ??
    currentInvocation(nodeInvocations, events, moment.index);
  const readingTranscript = useCallback(
    (reading: boolean) => {
      const id = activeInvocation?.id;
      if (id) setReadingExecution((previous) => (reading ? id : previous === id ? '' : previous));
    },
    [activeInvocation?.id]
  );
  const nodeControls = projection?.controls.filter((visit) => visit.node === node?.name) ?? [];
  const showExecutions =
    node && (['step', 'verifier'].includes(node.kind) || node.name === document?.graph.root.name);
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
            openLog={() => inspect(document.graph.root.name)}
            retry={() => void loadHistory(run.runId, source, false)}
            embedded={embedded}
            runHref={runHref}
            successorRunId={successorRunId}
          />
          <div className={`editor-body history-body ${inspector ? '' : 'inspector-closed'}`}>
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
                  observedNodes ?? projection.nodes,
                  isLiveRun && following
                )}
                view={canvas}
                onViewChange={setCanvas}
                inspectorHidden={!inspector}
                showInspector={() => setInspector(true)}
                deselect={() => inspect(document.graph.root.name, false)}
              />
            </ReactFlowProvider>
            {inspector && node && (
              <RunInspector
                run={run}
                document={document}
                projection={projection}
                terminal={displayedTerminal}
                currentFailure={currentFailure}
                node={node}
                moment={moment}
                nodeControls={nodeControls}
                nodeInvocations={nodeInvocations}
                activeInvocation={activeInvocation}
                showExecutions={!!showExecutions}
                controlError={controlError}
                execution={execution}
                playingNode={playing && playbackNode === node.name}
                canReplay={!loading && selectedReplay.length > 0}
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
                play={() => playNode()}
                restart={() => playNode(true)}
                reveal={reveal}
                inspectDiagnostic={inspectDiagnostic}
                hasLog={(cursor) => events.some((entry) => entry.cursor === cursor)}
                readingTranscript={readingTranscript}
              />
            )}
          </div>
          <RunTimeline
            moment={moment}
            moments={moments}
            safePosition={safePosition}
            isLiveRun={isLiveRun}
            following={following}
            connection={connection}
            loading={loading}
            loadedHead={loadedHead}
            playing={playing}
            historyPending={historyPending}
            restorePending={restorePending}
            eventsCount={events.length}
            showNode={() => reveal(moment.node!)}
            goLatest={goLatest}
            scrub={scrub}
            togglePlay={() => {
              failureView.replay();
              follow(false);
              if (safePosition === moments.length - 1) setCursorIndex(-1);
              setPlaybackNode(undefined);
              setExecution('');
              setReadingExecution('');
              setPlaying(!playing);
            }}
            loadMore={() => void loadHistory(run.runId, source, false)}
          />
        </>
      ) : (
        <HistoryEmpty
          historyPending={historyPending}
          loading={loading}
          error={error}
          runId={runId}
          emptyAction={emptyAction}
          retry={() => runId && void loadHistory(runId, source, true)}
        />
      )}
    </main>
  );
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
      const invocation = projection?.invocations.find((item) => item.id === String(execution));
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

/** The current time, renewed once when `deadline` passes so what waits on it can say so. */
function useDeadlineClock(deadline: number | undefined) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (deadline === undefined || deadline <= now) return;
    // Timers hold at most about 24.8 days; a later deadline renews the clock and waits again.
    const timer = setTimeout(() => setNow(Date.now()), Math.min(deadline - now, 2 ** 31 - 1));
    return () => clearTimeout(timer);
  }, [deadline, now]);
  return now;
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
            {successorRunId && runHref && (
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
      {run.observation && !['active', 'complete'].includes(run.observation.state) && (
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

type HistoryFeedCallbacks = {
  restore?: Partial<RunViewState>;
  onLoad: () => void;
  onReset: () => void;
  onOpen: (detail: RunDetail) => void;
  onMissing: () => void;
};
function useHistoryFeed(
  runId: string | undefined,
  source: RunHistoryReader,
  callbacks: HistoryFeedCallbacks
) {
  const [run, setRun] = useState<RunDetail>();
  const [events, setEvents] = useState<HistoryEvent[]>([]),
    [loadedHead, setLoadedHead] = useState(false);
  const [control, setControl] = useState<ControlRecord[]>([]),
    [controlError, setControlError] = useState('');
  const [cursorIndex, setCursorIndex] = useState(-1);
  const [following, setFollowing] = useState(false);
  const [connection, setConnection] = useState<
    'idle' | 'connecting' | 'connected' | 'reconnecting' | 'limited' | 'error'
  >('idle');
  const [loading, setLoading] = useState(false),
    [historyPending, setHistoryPending] = useState(false),
    [error, setError] = useState('');
  const loadGeneration = useRef(0),
    loadAbort = useRef<AbortController | undefined>(undefined);
  const liveAbort = useRef<AbortController | undefined>(undefined);
  const retained = useRef<HistoryEvent[]>([]),
    followLatest = useRef(false);
  const retainedControl = useRef<ControlRecord[]>([]);
  const cursorIntent = useRef(0);
  const pageCursor = useRef('v2:0');
  const restoreCursor = useRef<string | undefined>(undefined);
  const [restorePending, setRestorePending] = useState(false);
  const isLiveRun = !!run && canFollowHistory(run);
  async function loadHistory(id: string, dataSource: RunHistoryReader, reset: boolean) {
    const generation = ++loadGeneration.current;
    loadAbort.current?.abort();
    liveAbort.current?.abort();
    const controller = new AbortController();
    loadAbort.current = controller;
    setLoading(true);
    setHistoryPending(false);
    callbacks.onLoad();
    setError('');
    setConnection('idle');
    if (reset) {
      setRun(undefined);
      retainEvents([]);
      retainControl([]);
      setControlError('');
      setCursorIndex(-1);
      follow(false);
      restoreCursor.current = callbacks.restore?.cursor;
      setLoadedHead(false);
      callbacks.onReset();
      pageCursor.current = 'v2:0';
    }
    try {
      const pending = () => {
        if (!controller.signal.aborted && generation === loadGeneration.current)
          setHistoryPending(true);
      };
      const detail = await readHistoryWhenReady(
        () => dataSource.detail(id, controller.signal),
        controller.signal,
        dataSource.retryDelay,
        pending
      );
      if (controller.signal.aborted || generation !== loadGeneration.current) return;
      setHistoryPending(false);
      setRun(detail);
      if (reset) {
        callbacks.onOpen(detail);
        follow(
          (callbacks.restore?.following ?? true) && !!dataSource.watch && canFollowHistory(detail),
          true
        );
      }
      // A saved position owns its seek; later pages must not replace an already restored cursor.
      const initialIntent = restoreCursor.current ? undefined : cursorIntent.current;
      let collected = retained.current;
      let count = 0,
        bytes = 0,
        complete = false;
      while (!complete && count < 5000 && bytes < 8 * 1024 * 1024) {
        const page = await readHistoryWhenReady(
          () => dataSource.page(id, pageCursor.current, controller.signal),
          controller.signal,
          dataSource.retryDelay,
          pending
        );
        if (controller.signal.aborted || generation !== loadGeneration.current) return;
        setHistoryPending(false);
        if (!page.complete && (!page.events.length || page.nextCursor === pageCursor.current))
          throw new Error('History stopped advancing. Reload to try again.');
        collected = appendHistory(collected, page);
        acceptControl(page);
        count += page.events.length;
        bytes += JSON.stringify(page).length;
        pageCursor.current = page.nextCursor;
        complete = page.complete;
        retainEvents(collected);
        setLoadedHead(complete);
        if (
          (reset && cursorIntent.current === initialIntent) ||
          followLatest.current ||
          restoreCursor.current
        ) {
          const position = restoredHistoryPosition(
            followLatest.current ? undefined : restoreCursor.current,
            collected,
            complete
          );
          if (position === undefined) setRestorePending(true);
          else {
            setCursorIndex(position);
            restoreCursor.current = undefined;
            setRestorePending(false);
          }
        }
        acceptObservation(id, page);
        if (page.complete && page.finished) finishRun(id, collected, page);
      }
    } catch (error) {
      if (!controller.signal.aborted && generation === loadGeneration.current) {
        setHistoryPending(false);
        setError(message(error));
      }
    } finally {
      if (!controller.signal.aborted && generation === loadGeneration.current) setLoading(false);
    }
  }
  useEffect(() => {
    if (runId) void loadHistory(runId, source, true);
    else {
      loadAbort.current?.abort();
      liveAbort.current?.abort();
      setRun(undefined);
      retainEvents([]);
      retainControl([]);
      setControlError('');
      callbacks.onMissing();
      setLoading(false);
      setHistoryPending(false);
      setError('');
    }
    return () => {
      loadAbort.current?.abort();
      liveAbort.current?.abort();
    };
  }, [runId, source]);
  function retainEvents(next: HistoryEvent[]) {
    retained.current = next;
    setEvents(next);
    if (next.some(({ event }) => event.kind === 'suspend_requested'))
      setRun((previous) => (previous ? { ...previous, suspendRequested: true } : previous));
  }
  function retainControl(next: ControlRecord[]) {
    retainedControl.current = next;
    setControl(next);
  }
  function acceptControl(page: import('./run-history').HistoryPage) {
    try {
      retainControl(appendControlHistory(retainedControl.current, page, retained.current));
      if (page.controlError)
        setControlError(
          page.controlError === 'control_projection_limit'
            ? 'Control history reached its replay limit. Worker history remains available.'
            : 'Control history could not be reconstructed. Worker history remains available.'
        );
    } catch (error) {
      setControlError(message(error));
    }
  }
  function follow(value: boolean, restoring = false) {
    if (!restoring) {
      restoreCursor.current = undefined;
      setRestorePending(false);
    }
    cursorIntent.current++;
    followLatest.current = value;
    setFollowing(value);
  }
  function acceptObservation(id: string, page: HistoryPage) {
    if (page.observation)
      setRun((previous) =>
        previous?.runId === id ? { ...previous, observation: page.observation } : previous
      );
  }
  function finishRun(id: string, collected: HistoryEvent[], page: HistoryPage) {
    setRun((previous) =>
      previous?.runId === id ? finishRunHistory(previous, collected, page) : previous
    );
  }
  useEffect(() => {
    if (
      !source.watch ||
      !isLiveRun ||
      !run ||
      run.runId !== runId ||
      loading ||
      !loadedHead ||
      error ||
      connection === 'limited'
    )
      return;
    const controller = new AbortController();
    liveAbort.current = controller;
    const id = run.runId;
    let count = 0,
      bytes = 0;
    const dispose = source.watch(
      id,
      pageCursor.current,
      {
        page(page) {
          if (controller.signal.aborted) return;
          const collected = appendHistory(retained.current, page);
          acceptControl(page);
          const added = collected.length - retained.current.length;
          pageCursor.current = page.nextCursor;
          if (added) {
            count += added;
            bytes += JSON.stringify(page).length;
            retainEvents(collected);
            if (followLatest.current) setCursorIndex(collected.length - 1);
          }
          acceptObservation(id, page);
          if (page.complete && page.finished) finishRun(id, collected, page);
          if (page.complete && observationEnded(page.observation, page.finished === true)) {
            setConnection('idle');
          } else if (count >= 5000 || bytes >= 8 * 1024 * 1024) {
            controller.abort();
            setConnection('limited');
          }
        },
        status(status) {
          if (!controller.signal.aborted) setConnection(status);
        },
        error(error) {
          if (!controller.signal.aborted) {
            setError(message(error));
            setConnection('error');
          }
        },
      },
      controller.signal
    );
    return () => {
      controller.abort();
      dispose();
    };
  }, [source, isLiveRun, run?.runId, runId, loading, loadedHead, error, connection === 'limited']);
  return {
    run,
    events,
    loadedHead,
    control,
    controlError,
    cursorIndex,
    setCursorIndex,
    following,
    connection,
    loading,
    historyPending,
    error,
    retained,
    restoreCursor,
    restorePending,
    isLiveRun,
    follow,
    loadHistory,
  };
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
                {run.runtime.harness} / {run.runtime.provider}
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
        {!nodeControls.length && observedRoutes(node, projection.invocations).length > 0 && (
          <section className="run-inspector-section">
            <h3>Observed paths</h3>
            {observedRoutes(node, projection.invocations).map((route) => (
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
            node.name === document.graph.root.name ? run.runtime : run.runtime.nodes[node.name]
          }
          raw
        />
      </div>
    </InspectorPanel>
  );
}

function InvocationView({
  invocation,
  showNode,
  reveal,
  follow,
  reading,
}: {
  invocation: Invocation;
  showNode: boolean;
  reveal: () => void;
  follow: boolean;
  reading: (value: boolean) => void;
}) {
  const outcome = invocation.outcome;
  return (
    <section className="invocation-detail">
      <div className="invocation-status">
        <span className={`run-status state-${invocation.state}`}>
          {stateLabel(invocation.state)}
        </span>
      </div>
      {showNode && (
        <button className="invocation-node" onClick={reveal}>
          {humanize(invocation.node)}
          <ChevronRight size={15} />
        </button>
      )}
      <ValueSection title="Input" value={invocation.input} />
      {outcome && (
        <details className="history-value invocation-output" open>
          <summary>Output</summary>
          {outcome.signals && Object.keys(outcome.signals).length > 0 && (
            <div className="recorded-signals">
              {Object.entries(outcome.signals).map(([key, value]) => (
                <div key={key}>
                  <span>{humanize(key)}</span>
                  <strong>{humanize(value)}</strong>
                </div>
              ))}
            </div>
          )}
          {outcome.status === 'error' && (
            <div className="history-node-error">
              {humanize(outcome.code ?? 'Error')}
              {outcome.reason && outcome.reason !== 'declared_failure' && (
                <small>{humanize(outcome.reason)}</small>
              )}
            </div>
          )}
          {outcome.output != null && <RecordedValue value={outcome.output} />}
          {outcome.diagnostic != null && (
            <div className="outcome-diagnostic">
              {outcome.output != null && <h4>Details</h4>}
              <RecordedValue value={outcome.diagnostic} />
            </div>
          )}
          {!!outcome.artifacts?.length && (
            <RecordedValue value={{ artifacts: outcome.artifacts }} />
          )}
          {outcome.status !== 'error' &&
            outcome.output == null &&
            outcome.diagnostic == null &&
            !Object.keys(outcome.signals ?? {}).length &&
            !outcome.artifacts?.length && <p className="history-muted">No returned value.</p>}
        </details>
      )}
      {invocation.reason && (
        <ValueSection title="Stopped because" value={humanize(invocation.reason)} open />
      )}
      <Transcript logs={invocation.logs} follow={follow} onReadingChange={reading} />
    </section>
  );
}
function ValueSection({
  title,
  value,
  open = false,
  raw = false,
}: {
  title: string;
  value: unknown;
  open?: boolean;
  raw?: boolean;
}) {
  if (value === undefined || value === null) return null;
  return (
    <details className="history-value" open={open || undefined}>
      <summary>{title}</summary>
      {raw ? <pre>{JSON.stringify(value, null, 2)}</pre> : <RecordedValue value={value} />}
    </details>
  );
}
