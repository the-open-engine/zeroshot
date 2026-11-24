import { useEffect, useRef, useState, type Dispatch, type SetStateAction } from 'react';
import { appendControlHistory, type ControlRecord } from './control-history';
import { groupedControlPage } from './grouped-history';
import { canFollowHistory, observationEnded } from './history-contract';
import { readHistoryWhenReady } from './history-readiness';
import {
  finishRunHistory,
  type RunDetail,
  type HistoryPage,
  type HistoryEvent,
} from './run-history';
import { appendHistory, type RunHistoryReader } from './run-history-source';
import { restoredHistoryPosition, type RunViewState } from './run-view-state';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

function needsDetail(page: HistoryPage, head?: string, cursor?: string) {
  if (!page.partAfterCursor || page.partId !== page.currentPartId) return false;
  return (
    head !== page.currentPartId || (page.complete && page.finished && cursor !== page.headCursor)
  );
}

function matchesRun(run: RunDetail, id?: string) {
  return id === run.runId || (!!id && run.parts?.includes(id));
}

function requireProgress(page: HistoryPage, after: string) {
  if (
    !page.complete &&
    ((!page.partAfterCursor && !page.events.length) || page.nextCursor === after)
  )
    throw new Error('History stopped advancing. Reload to try again.');
}

function pageRecordCount(page: HistoryPage) {
  return page.events.length + (page.imports?.length ?? 0) + (page.omittedCursors?.length ?? 0);
}

function applyObservation(previous: RunDetail | undefined, root: string, page: HistoryPage) {
  if (!previous || previous.runId !== root) return previous;
  let next = { ...previous, observation: page.observation ?? previous.observation };
  if (!page.partAfterCursor || page.partId !== page.currentPartId) return next;
  if (next.currentPartId !== page.currentPartId) {
    next = {
      ...next,
      currentPartId: page.currentPartId,
      partId: page.partId,
      parts: [...(next.parts ?? []), page.partId!],
      phase: 'running',
      suspension: undefined,
      terminal: null,
      runtimeFailure: undefined,
      suspendRequested: false,
      workspaceRecovery: undefined,
    };
  }
  for (const { event } of page.events) {
    if (event.kind === 'suspended')
      next = { ...next, phase: 'suspended', suspension: event.suspension, suspendRequested: false };
    else if (event.kind === 'suspend_requested') next = { ...next, suspendRequested: true };
  }
  return next;
}

/** Head configuration and cumulative counters can change when a new part opens or closes.
 * Lifecycle state still comes from the pages delivered to the active subscription. */
function useHeadMetadata(
  source: RunHistoryReader,
  setRun: Dispatch<SetStateAction<RunDetail | undefined>>,
  setError: (message: string) => void
) {
  const detailHead = useRef<string | undefined>(undefined);
  const detailCursor = useRef<string | undefined>(undefined);
  function remember(detail: RunDetail) {
    detailHead.current = detail.currentPartId;
    detailCursor.current = detail.cursor;
  }
  function refresh(root: string, page: HistoryPage, controller: AbortController | undefined) {
    if (page.currentPartId && needsDetail(page, detailHead.current, detailCursor.current)) {
      const head = page.currentPartId;
      const cursor = page.headCursor;
      detailHead.current = head;
      detailCursor.current = cursor;
      void source
        .detail(root, controller?.signal)
        .then((detail) => {
          if (
            controller?.signal.aborted ||
            detailHead.current !== head ||
            detailCursor.current !== cursor ||
            detail.currentPartId !== head
          )
            return;
          // The definition and cumulative usage are current at the host. The subscription owns
          // lifecycle state until it has actually delivered the terminal page.
          setRun((previous) =>
            previous?.runId === root
              ? {
                  ...previous,
                  parts: detail.parts,
                  graph: detail.graph,
                  runtime: detail.runtime,
                  metadata: detail.metadata,
                  environment: detail.environment,
                  workspaceRecovery: detail.workspaceRecovery,
                }
              : previous
          );
        })
        .catch((cause) => {
          if (!controller?.signal.aborted) setError(message(cause));
        });
    }
  }
  return { remember, refresh };
}

type HistoryFeedCallbacks = {
  restore?: Partial<RunViewState>;
  onLoad: () => void;
  onReset: () => void;
  onOpen: (detail: RunDetail) => void;
  onMissing: () => void;
};
export function useHistoryFeed(
  runId: string | undefined,
  source: RunHistoryReader,
  callbacks: HistoryFeedCallbacks,
  listedPartId?: string
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
  const metadata = useHeadMetadata(source, setRun, setError);
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
      metadata.remember(detail);
      if (reset) {
        pageCursor.current = detail.history.initialCursor;
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
        requireProgress(page, pageCursor.current);
        collected = appendHistory(collected, page, pageCursor.current);
        acceptControl(page);
        count += pageRecordCount(page);
        bytes += JSON.stringify(page).length;
        pageCursor.current = page.nextCursor;
        complete = page.complete;
        retainEvents(collected);
        setLoadedHead(complete);
        if (
          (reset && cursorIntent.current === initialIntent) ||
          followLatest.current ||
          restoreCursor.current
        )
          restorePosition(collected, complete);
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
  function restorePosition(collected: HistoryEvent[], complete: boolean) {
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
  useEffect(() => {
    if (
      listedPartId &&
      runId &&
      run?.phase === 'finished' &&
      !run.partView &&
      run.parts &&
      !run.parts.includes(listedPartId) &&
      !loading &&
      !error
    ) {
      void loadHistory(runId, source, false);
    }
  }, [listedPartId, runId, source, run?.currentPartId, run?.phase]);
  function retainEvents(next: HistoryEvent[]) {
    retained.current = next;
    setEvents(next);
    if (!next.at(-1)?.partId && next.some(({ event }) => event.kind === 'suspend_requested'))
      setRun((previous) => (previous ? { ...previous, suspendRequested: true } : previous));
  }
  function retainControl(next: ControlRecord[]) {
    retainedControl.current = next;
    setControl(next);
  }
  function acceptControl(page: HistoryPage) {
    try {
      retainControl(
        appendControlHistory(
          retainedControl.current,
          page.partAfterCursor ? groupedControlPage(page) : page,
          retained.current
        )
      );
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
    const root = page.runId ?? id;
    metadata.refresh(root, page, loadAbort.current);
    setRun((previous) => applyObservation(previous, root, page));
  }
  function finishRun(id: string, collected: HistoryEvent[], page: HistoryPage) {
    setRun((previous) =>
      previous?.runId === (page.runId ?? id)
        ? finishRunHistory(previous, collected, page)
        : previous
    );
  }
  useEffect(() => {
    if (
      !source.watch ||
      !isLiveRun ||
      !run ||
      !matchesRun(run, runId) ||
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
          const collected = appendHistory(retained.current, page, pageCursor.current);
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
