import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import {
  AlertCircle,
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  Clock3,
  Loader2,
  Pause,
  Play,
  X,
} from 'lucide-react';
import { ExecutionTimeline } from './ExecutionTimeline';
import { ControlTimeline } from './ControlTimeline';
import { appendControlHistory, type ControlRecord } from './control-history';
import { RecordedValue } from './RecordedValue';
import { Transcript } from './Transcript';
import { WorkflowCanvas } from './WorkflowCanvas';
import { findNode, labels, type Document, type Positions } from './domain';
import type { WorkerOption } from './workers';
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
  stateLabel,
  type HistoryEvent,
  type HistoryPage,
  type Invocation,
  type RunDetail,
} from './run-history';
import { canFollowHistory, observationEnded } from './history-contract';
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
export function RunHistoryView({
  runId,
  source,
  workers,
  emptyAction,
}: {
  runId?: string;
  source: RunHistoryReader;
  workers: WorkerOption[];
  emptyAction?: ReactNode;
}) {
  const [run, setRun] = useState<RunDetail>();
  const [events, setEvents] = useState<HistoryEvent[]>([]),
    [loadedHead, setLoadedHead] = useState(false);
  const [control, setControl] = useState<ControlRecord[]>([]),
    [controlError, setControlError] = useState('');
  const [cursorIndex, setCursorIndex] = useState(-1),
    [playing, setPlaying] = useState(false);
  const [following, setFollowing] = useState(false);
  const [connection, setConnection] = useState<
    'idle' | 'connecting' | 'connected' | 'reconnecting' | 'limited' | 'error'
  >('idle');
  const [playbackNode, setPlaybackNode] = useState<string>();
  const [selected, setSelected] = useState(''),
    [inspector, setInspector] = useState(false),
    [execution, setExecution] = useState('');
  const [readingExecution, setReadingExecution] = useState('');
  const [positions, setPositions] = useState<Positions>({}),
    [focus, setFocus] = useState<{ name: string; tick: number }>();
  const [loading, setLoading] = useState(false),
    [error, setError] = useState('');
  const loadGeneration = useRef(0),
    loadAbort = useRef<AbortController | undefined>(undefined);
  const liveAbort = useRef<AbortController | undefined>(undefined);
  const retained = useRef<HistoryEvent[]>([]),
    followLatest = useRef(false);
  const retainedControl = useRef<ControlRecord[]>([]);
  const cursorIntent = useRef(0);
  const pageCursor = useRef('v2:0');
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
  const isLiveRun = !!run && canFollowHistory(run);
  const projection = useMemo(
    () => document && projectHistory(document, events, moment.index, control),
    [document, events, moment.index, control]
  );
  useEffect(() => {
    window.document.title = `${run?.title ?? 'Runs'} · Zeroshot`;
  }, [run?.title]);

  async function loadHistory(id: string, dataSource: RunHistoryReader, reset: boolean) {
    const generation = ++loadGeneration.current;
    loadAbort.current?.abort();
    liveAbort.current?.abort();
    const controller = new AbortController();
    loadAbort.current = controller;
    setLoading(true);
    setPlaying(false);
    setPlaybackNode(undefined);
    setError('');
    setConnection('idle');
    if (reset) {
      setRun(undefined);
      retainEvents([]);
      retainControl([]);
      setControlError('');
      setCursorIndex(-1);
      follow(false);
      setLoadedHead(false);
      setInspector(false);
      setExecution('');
      setReadingExecution('');
      setFocus(undefined);
      setPositions({});
      pageCursor.current = 'v2:0';
    }
    try {
      const detail = await dataSource.detail(id, controller.signal);
      if (controller.signal.aborted || generation !== loadGeneration.current) return;
      setRun(detail);
      if (reset) {
        setSelected(detail.graph.root.name);
        follow(!!dataSource.watch && canFollowHistory(detail));
      }
      const initialIntent = cursorIntent.current;
      let collected = retained.current;
      let count = 0,
        bytes = 0,
        complete = false;
      while (!complete && count < 5000 && bytes < 8 * 1024 * 1024) {
        const page = await dataSource.page(id, pageCursor.current, controller.signal);
        if (controller.signal.aborted || generation !== loadGeneration.current) return;
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
        if ((reset && cursorIntent.current === initialIntent) || followLatest.current)
          setCursorIndex(collected.length - 1);
        acceptObservation(id, page);
        if (page.complete && page.finished) finishRun(id, collected, page);
      }
    } catch (error) {
      if (!controller.signal.aborted && generation === loadGeneration.current)
        setError(message(error));
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
      setPlaying(false);
      setLoading(false);
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
  function follow(value: boolean) {
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
  useEffect(() => {
    if (!playing) return;
    const next = nextReplayPosition(safePosition, moments.length, replayScope);
    if (next === undefined) {
      setPlaying(false);
      return;
    }
    const timer = setTimeout(() => {
      setCursorIndex(moments[next].index);
      if (nextReplayPosition(next, moments.length, replayScope) === undefined) setPlaying(false);
    }, 950);
    return () => clearTimeout(timer);
    // Each step owns its checkpoint until the timer fires; incoming live logs cannot postpone it.
  }, [playing, cursorIndex, playbackNode]);
  function scrub(next: number) {
    setPlaying(false);
    follow(false);
    setExecution('');
    setReadingExecution('');
    setCursorIndex(moments[Math.max(0, Math.min(moments.length - 1, next))].index);
  }
  function goLatest() {
    setPlaying(false);
    setPlaybackNode(undefined);
    setExecution('');
    setReadingExecution('');
    follow(isLiveRun);
    setCursorIndex(retained.current.length - 1);
    if (run && (connection === 'limited' || connection === 'error' || (!loading && !loadedHead)))
      void loadHistory(run.runId, source, false);
  }
  function inspect(name: string) {
    setPlaying(false);
    setPlaybackNode(undefined);
    setSelected(name);
    setExecution('');
    setReadingExecution('');
    setInspector(true);
  }
  function playNode(restart = false) {
    if (!selectedReplay.length) return;
    follow(false);
    if (!restart && playing && playbackNode === selected) {
      setPlaying(false);
      return;
    }
    const first = selectedReplay[0],
      last = selectedReplay.at(-1)!;
    const start = restart || safePosition < first || safePosition >= last ? first : safePosition;
    setCursorIndex(moments[start].index);
    setExecution('');
    setReadingExecution('');
    setPlaybackNode(selected);
    setPlaying(nextReplayPosition(start, moments.length, selectedReplay) !== undefined);
  }
  function reveal(name: string) {
    inspect(name);
    setFocus({ name, tick: Date.now() });
  }
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
  const currentState =
    projection?.terminal?.status ??
    (loadedHead && run?.runtimeFailure
      ? 'failed'
      : safePosition === 0
        ? 'Before run'
        : loadedHead && run?.phase === 'finished'
          ? 'Recorded history'
          : 'In progress');
  const shownStatus = safePosition === moments.length - 1 && loadedHead ? currentState : 'Replay';

  return (
    <main className="workspace history-workspace">
      {run && document && projection ? (
        <>
          <div className="workspace-header history-header">
            <div className="profile-title">
              <h1 title={run.title}>{run.title}</h1>
              <span
                className={`run-status state-${projection.terminal?.status ?? (shownStatus === 'failed' ? 'failed' : 'replay')}`}
              >
                {run.example ? 'Simulated' : humanize(shownStatus)}
              </span>
            </div>
          </div>
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
              <AlertCircle size={16} />
              <span>
                <strong>History incomplete.</strong> Runtime failed before its final state was
                saved.
              </span>
              <button
                className="text-button"
                disabled={loading}
                onClick={() => void loadHistory(run.runId, source, false)}
              >
                Reload
              </button>
            </div>
          )}
          {error && (
            <div className="history-error" role="alert">
              <AlertCircle size={16} />
              <span>{error}</span>
              <button
                className="text-button"
                onClick={() => void loadHistory(run.runId, source, false)}
              >
                Retry
              </button>
            </div>
          )}
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
                observation={{ key: run.runId, nodes: projection.nodes }}
                inspectorHidden={!inspector}
                showInspector={() => setInspector(true)}
              />
            </ReactFlowProvider>
            {inspector && node && (
              <aside className="run-inspector" aria-label="Run inspector">
                <header className="run-inspector-header">
                  <div>
                    <span className="eyebrow">
                      {node.name === document.graph.root.name ? 'RUN OVERVIEW' : labels[node.kind]}
                    </span>
                    <h2>
                      {node.name === document.graph.root.name ? 'Run details' : humanize(node.name)}
                    </h2>
                  </div>
                  <button
                    className="icon-button"
                    title="Close inspector"
                    aria-label="Close inspector"
                    onClick={() => {
                      setInspector(false);
                      if (playbackNode) setPlaying(false);
                    }}
                  >
                    <X size={17} />
                  </button>
                </header>
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
                      {projection.terminal && (
                        <ValueSection
                          title={
                            projection.terminal.status === 'succeeded'
                              ? 'Run output'
                              : 'Failure reason'
                          }
                          value={
                            projection.terminal.status === 'succeeded'
                              ? projection.terminal.output
                              : projection.terminal.reason
                          }
                          open
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
                  {!nodeControls.length &&
                    observedRoutes(node, projection.invocations).length > 0 && (
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
                        select={(id) => {
                          setPlaying(false);
                          follow(false);
                          setReadingExecution('');
                          setExecution(id);
                        }}
                        reveal={reveal}
                        playing={playing && playbackNode === node.name}
                        canReplay={!loading && selectedReplay.length > 0}
                        play={() => playNode()}
                        restart={() => playNode(true)}
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
                        select={(id) => {
                          setPlaying(false);
                          follow(false);
                          setReadingExecution('');
                          setExecution(id);
                        }}
                        playing={playing && playbackNode === node.name}
                        canReplay={!loading && selectedReplay.length > 0}
                        play={() => playNode()}
                        restart={() => playNode(true)}
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
                        ? run.runtime
                        : run.runtime.nodes[node.name]
                    }
                    raw
                  />
                </div>
              </aside>
            )}
          </div>
          <footer className="history-timeline" aria-label="Run timeline">
            <div className="timeline-caption">
              <div>
                {!(isLiveRun && following) && (
                  <span className="eyebrow">
                    {safePosition === 0
                      ? 'START'
                      : safePosition === moments.length - 1 && loadedHead
                        ? 'LATEST'
                        : 'REPLAY'}
                  </span>
                )}
                <strong>{moment.label}</strong>
                {moment.node && (
                  <button className="text-button" onClick={() => reveal(moment.node!)}>
                    Show node
                  </button>
                )}
              </div>
              <div className="timeline-progress">
                {isLiveRun && (
                  <button
                    className={`history-live ${following && connection === 'connected' ? 'following' : ''}`}
                    disabled={loading || (following && connection === 'connected')}
                    title={following ? 'Following latest events' : 'Follow latest events'}
                    onClick={goLatest}
                  >
                    <span className="live-dot" />
                    {connection === 'reconnecting'
                      ? 'Reconnecting'
                      : connection === 'connecting'
                        ? 'Connecting'
                        : connection === 'limited' || (!loading && !loadedHead)
                          ? 'Load more'
                          : connection === 'error'
                            ? 'Reconnect'
                            : loading || connection === 'idle'
                              ? 'Connecting'
                              : following
                                ? 'Live'
                                : 'Go live'}
                  </button>
                )}
                <span className="timeline-counter">
                  {safePosition} / {moments.length - 1}
                </span>
              </div>
            </div>
            <div className="timeline-controls">
              <button
                className="icon-button"
                title="Go to start"
                aria-label="Go to start"
                disabled={!safePosition}
                onClick={() => scrub(0)}
              >
                <ChevronsLeft size={18} />
              </button>
              <button
                className="icon-button"
                title="Previous event"
                aria-label="Previous event"
                disabled={!safePosition}
                onClick={() => scrub(safePosition - 1)}
              >
                <ArrowLeft size={17} />
              </button>
              <button
                className="timeline-play"
                title={playing ? 'Pause replay' : 'Play replay'}
                aria-label={playing ? 'Pause replay' : 'Play replay'}
                disabled={moments.length < 2 || loading}
                onClick={() => {
                  follow(false);
                  if (safePosition === moments.length - 1) setCursorIndex(-1);
                  setPlaybackNode(undefined);
                  setExecution('');
                  setReadingExecution('');
                  setPlaying(!playing);
                }}
              >
                {playing ? <Pause size={17} /> : <Play size={17} />}
              </button>
              <input
                type="range"
                aria-label="History position"
                aria-valuetext={moment.label}
                min={0}
                max={moments.length - 1}
                step={1}
                value={safePosition}
                onChange={(event) => scrub(Number(event.target.value))}
              />
              <button
                className="icon-button"
                title="Next event"
                aria-label="Next event"
                disabled={safePosition === moments.length - 1}
                onClick={() => scrub(safePosition + 1)}
              >
                <ArrowRight size={17} />
              </button>
              <button
                className="icon-button"
                title="Go to latest"
                aria-label="Go to latest"
                disabled={safePosition === moments.length - 1 && (!isLiveRun || following)}
                onClick={goLatest}
              >
                <ChevronsRight size={18} />
              </button>
            </div>
            {loading ? (
              <div className="history-load-state">
                <Loader2 size={13} className="spin" /> Loading history · {events.length} events
              </div>
            ) : !loadedHead || connection === 'limited' ? (
              <div className="history-load-state">
                {events.length} events loaded{' '}
                <button
                  className="text-button"
                  onClick={() => void loadHistory(run.runId, source, false)}
                >
                  Load more history
                </button>
              </div>
            ) : null}
          </footer>
        </>
      ) : (
        <div className="history-empty">
          {loading ? (
            <>
              <Loader2 size={23} className="spin" />
              <h1>Loading run</h1>
            </>
          ) : error ? (
            <>
              <AlertCircle size={25} />
              <h1>Could not open this run</h1>
              <p>{error}</p>
              {runId && (
                <button className="button" onClick={() => void loadHistory(runId, source, true)}>
                  Retry
                </button>
              )}
            </>
          ) : (
            <>
              <Clock3 size={30} strokeWidth={1.3} />
              <h1>Run history</h1>
              <p>Select a run to inspect its graph and recorded history.</p>
              {emptyAction}
            </>
          )}
        </div>
      )}
    </main>
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
          {invocation.state === 'succeeded' && <Check size={12} />}
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
