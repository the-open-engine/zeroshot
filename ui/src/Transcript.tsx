import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { Invocation } from './run-history';
import {
  atTranscriptBottom,
  transcriptRange,
  TRANSCRIPT_PAGE_SIZE,
  type TranscriptWindow,
} from './transcript-window';

export function Transcript({
  logs,
  title = 'Transcript',
  follow = false,
  onReadingChange,
}: {
  logs: Invocation['logs'];
  title?: string;
  follow?: boolean;
  onReadingChange?: (reading: boolean) => void;
}) {
  const [filter, setFilter] = useState('all');
  const [window, setWindow] = useState<TranscriptWindow>({
    start: 0,
    count: TRANSCRIPT_PAGE_SIZE,
    atBottom: true,
  });
  const scrollBox = useRef<HTMLDivElement>(null);
  const filtered = logs.filter((log) => filter === 'all' || log.stream === 'output');
  const { start, end } = transcriptRange(filtered.length, window, follow);
  const lastIndex = filtered.at(-1)?.index;
  useEffect(() => onReadingChange?.(!window.atBottom), [window.atBottom, onReadingChange]);

  useLayoutEffect(() => {
    // Remember a clamped window after seeking backward, including an empty prefix.
    setWindow((current) => (current.start === start ? current : { ...current, start }));
    if (follow && window.atBottom && scrollBox.current) {
      scrollBox.current.scrollTop = scrollBox.current.scrollHeight;
    }
  }, [follow, window.atBottom, start, end, lastIndex, filter]);

  const pin = () => setWindow((current) => ({ ...current, start, atBottom: false }));

  return (
    <section className="history-transcript">
      <div className="transcript-heading">
        <h3>
          {title} <span>{logs.length}</span>
        </h3>
        {logs.some((log) => log.stream !== 'output') && (
          <select
            aria-label="Transcript filter"
            value={filter}
            onChange={(event) => {
              setFilter(event.target.value);
              setWindow((current) => ({
                ...current,
                start: 0,
                count: TRANSCRIPT_PAGE_SIZE,
              }));
            }}
          >
            <option value="all">All output</option>
            <option value="messages">Messages</option>
          </select>
        )}
      </div>
      <div
        ref={scrollBox}
        className="transcript-scroll"
        role="region"
        aria-label={`${title} entries`}
        tabIndex={0}
        onScroll={(event) => {
          const atBottom = atTranscriptBottom(event.currentTarget);
          setWindow((current) =>
            current.atBottom === atBottom && current.start === start
              ? current
              : { ...current, start, atBottom }
          );
        }}
      >
        {start > 0 && (
          <button
            className="text-button history-more"
            onClick={() =>
              setWindow((current) => ({
                start: Math.max(0, start - TRANSCRIPT_PAGE_SIZE),
                count: current.count + Math.min(start, TRANSCRIPT_PAGE_SIZE),
                atBottom: false,
              }))
            }
          >
            Show earlier output ({start})
          </button>
        )}
        {!filtered.length && (
          <p className="history-muted">
            No {filter === 'messages' ? 'messages' : 'output'} recorded at this point.
          </p>
        )}
        {filtered.slice(start, end).map((log) => (
          <LogEntry key={log.index} log={log} read={pin} />
        ))}
        {filtered.length > end && (
          <button
            className="text-button history-more"
            onClick={() =>
              setWindow((current) => ({
                start,
                count: current.count + TRANSCRIPT_PAGE_SIZE,
                atBottom: false,
              }))
            }
          >
            Show more output ({filtered.length - end})
          </button>
        )}
      </div>
    </section>
  );
}

function LogEntry({ log, read }: { log: Invocation['logs'][number]; read: () => void }) {
  const [expanded, setExpanded] = useState(false);
  const long = log.text.length > 1200 || log.text.split('\n').length > 14;
  const preview = log.text.split('\n').slice(0, 9).join('\n').slice(0, 1200);
  return (
    <article className={`transcript-entry stream-${log.stream}`}>
      <header>
        <span>
          {log.stream === 'output' ? 'Agent' : log.stream === 'error' ? 'Error' : 'Activity'}
        </span>
        {log.timestamp && (
          <time>
            {new Date(log.timestamp).toLocaleTimeString(undefined, {
              hour: '2-digit',
              minute: '2-digit',
              second: '2-digit',
            })}
          </time>
        )}
      </header>
      <pre>{long && !expanded ? `${preview}\n…` : log.text}</pre>
      {long && (
        <button
          className="text-button"
          onClick={() => {
            read();
            setExpanded(!expanded);
          }}
        >
          {expanded
            ? 'Show less'
            : `Show full output (${log.text.length.toLocaleString()} characters)`}
        </button>
      )}
    </article>
  );
}
