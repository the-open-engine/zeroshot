import type { CSSProperties } from 'react';
import {
  ArrowLeft,
  ArrowRight,
  ChevronsLeft,
  ChevronsRight,
  Loader2,
  Pause,
  Play,
} from 'lucide-react';
import type { historyMoments } from './run-history';

type RunTimelineProps = {
  moment: ReturnType<typeof historyMoments>[number];
  moments: ReturnType<typeof historyMoments>;
  safePosition: number;
  isLiveRun: boolean;
  following: boolean;
  connection: 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'limited' | 'error';
  loading: boolean;
  loadedHead: boolean;
  playing: boolean;
  historyPending: boolean;
  restorePending: boolean;
  eventsCount: number;
  showNode: () => void;
  goLatest: () => void;
  scrub: (index: number) => void;
  togglePlay: () => void;
  loadMore: () => void;
};

function liveStatusLabel(
  connection: RunTimelineProps['connection'],
  loading: boolean,
  loadedHead: boolean,
  following: boolean
): string {
  return connection === 'reconnecting'
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
              : 'Go live';
}

/** Replay, as one slim line under the view: step controls, a thin track, and where it stands. */
export function RunTimeline({
  moment,
  moments,
  safePosition,
  isLiveRun,
  following,
  connection,
  loading,
  loadedHead,
  playing,
  historyPending,
  restorePending,
  eventsCount,
  showNode,
  goLatest,
  scrub,
  togglePlay,
  loadMore,
}: RunTimelineProps) {
  const last = moments.length - 1;
  return (
    <footer className="history-timeline" aria-label="Run timeline">
      <div className="timeline-bar">
        <div className="timeline-controls">
          <button
            className="icon-button"
            title="Go to start"
            aria-label="Go to start"
            disabled={!safePosition}
            onClick={() => scrub(0)}
          >
            <ChevronsLeft size={16} />
          </button>
          <button
            className="icon-button"
            title="Previous event"
            aria-label="Previous event"
            disabled={!safePosition}
            onClick={() => scrub(safePosition - 1)}
          >
            <ArrowLeft size={15} />
          </button>
          <button
            className="icon-button"
            title={playing ? 'Pause replay' : 'Play replay'}
            aria-label={playing ? 'Pause replay' : 'Play replay'}
            disabled={moments.length < 2 || loading}
            onClick={togglePlay}
          >
            {playing ? <Pause size={15} /> : <Play size={15} />}
          </button>
          <button
            className="icon-button"
            title="Next event"
            aria-label="Next event"
            disabled={safePosition === last}
            onClick={() => scrub(safePosition + 1)}
          >
            <ArrowRight size={15} />
          </button>
          <button
            className="icon-button"
            title="Go to latest"
            aria-label="Go to latest"
            disabled={safePosition === last && loadedHead && (!isLiveRun || following)}
            onClick={goLatest}
          >
            <ChevronsRight size={16} />
          </button>
        </div>
        <input
          type="range"
          aria-label="History position"
          aria-valuetext={moment.label}
          min={0}
          max={last}
          step={1}
          value={safePosition}
          style={
            { '--progress': `${last > 0 ? (safePosition / last) * 100 : 0}%` } as CSSProperties
          }
          onChange={(event) => scrub(Number(event.target.value))}
        />
        {/* The event and its extras share one fixed-width column: the track keeps its length and
            the event name starts in the same place while dragging; the name gives way first. */}
        <div className="timeline-info">
          <div className="timeline-caption">
            <strong title={moment.label}>{moment.label}</strong>
            {!(isLiveRun && following) && (
              <span className="timeline-mode">
                {safePosition === 0
                  ? 'Start'
                  : safePosition === last && loadedHead
                    ? 'Latest'
                    : 'Replay'}
              </span>
            )}
            <span className="timeline-counter">
              {safePosition} / {last}
            </span>
          </div>
          <div className="timeline-extras">
            {isLiveRun && (
              <LiveStatus
                connection={connection}
                loading={loading}
                loadedHead={loadedHead}
                following={following}
                goLatest={goLatest}
              />
            )}
            {moment.node && (
              <button className="text-button" onClick={showNode}>
                Show node
              </button>
            )}
          </div>
        </div>
      </div>
      {historyPending ? (
        <div className="history-load-state" role="status">
          <Loader2 size={13} className="spin" /> Run history is still setting up
        </div>
      ) : loading ? (
        <div className="history-load-state">
          <Loader2 size={13} className="spin" /> Loading history · {eventsCount} events
        </div>
      ) : !loadedHead || connection === 'limited' ? (
        <div className="history-load-state">
          {restorePending
            ? 'Load more history to restore your saved position.'
            : `${eventsCount} events loaded`}{' '}
          <button className="text-button" onClick={loadMore}>
            Load more history
          </button>
        </div>
      ) : null}
    </footer>
  );
}

/** Whether the run is live, and whether the view keeps up with it; a way back when it does not. */
function LiveStatus({
  connection,
  loading,
  loadedHead,
  following,
  goLatest,
}: Pick<RunTimelineProps, 'connection' | 'loading' | 'loadedHead' | 'following' | 'goLatest'>) {
  const current = following && connection === 'connected';
  return (
    <button
      className={`history-live ${current ? 'following' : ''}`}
      disabled={loading || current}
      title={following ? 'Following latest events' : 'Follow latest events'}
      onClick={goLatest}
    >
      <span className="live-dot" />
      {liveStatusLabel(connection, loading, loadedHead, following)}
    </button>
  );
}
