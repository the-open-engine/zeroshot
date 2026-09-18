import { useEffect, useRef } from 'react';
import { Check, Circle, Minus, Pause, Play, RotateCcw, X } from 'lucide-react';
import type { Document } from './domain';
import {
  humanize,
  invocationLabel,
  stateLabel,
  type Invocation,
  type NodeState,
} from './run-history';

/** An ordered strip of executions from the selected history prefix. */
export function ExecutionTimeline({
  invocations,
  selected,
  node,
  document,
  select,
  playing,
  canReplay,
  play,
  restart,
}: {
  invocations: Invocation[];
  selected?: string;
  node: string;
  document: Document;
  select: (id: string) => void;
  playing: boolean;
  canReplay: boolean;
  play: () => void;
  restart: () => void;
}) {
  const entries = invocations.map((invocation) => {
    const label = invocationLabel(invocation, document);
    const name = invocation.node !== node ? humanize(invocation.node) : '';
    const decision = Object.entries(invocation.outcome?.signals ?? {})
      .map(([key, value]) => `${humanize(key)}: ${humanize(value)}`)
      .join(' · ');
    return {
      id: invocation.id,
      label,
      name,
      state: invocation.state,
      description: [name, label, stateLabel(invocation.state), decision]
        .filter(Boolean)
        .join(' · '),
    };
  });
  return (
    <ActivityTimeline
      title="Executions"
      {...{ entries, selected, select, playing, canReplay, play, restart }}
    />
  );
}

/** Shared, keyboard-accessible history strip for worker executions and control visits. */
export function ActivityTimeline({
  title,
  entries,
  selected,
  select,
  playing,
  canReplay,
  play,
  restart,
}: {
  title: string;
  entries: { id: string; label: string; name?: string; state: NodeState; description: string }[];
  selected?: string;
  select: (id: string) => void;
  playing: boolean;
  canReplay: boolean;
  play: () => void;
  restart: () => void;
}) {
  const track = useRef<HTMLOListElement>(null);
  const active = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const list = track.current,
      button = active.current;
    if (!list || !button) return;
    const left =
        button.getBoundingClientRect().left - list.getBoundingClientRect().left + list.scrollLeft,
      right = left + button.offsetWidth;
    if (left < list.scrollLeft) list.scrollLeft = left;
    else if (right > list.scrollLeft + list.clientWidth) list.scrollLeft = right - list.clientWidth;
  }, [selected]);
  return (
    <section className="execution-timeline" aria-label={title}>
      <div className="execution-timeline-heading">
        <h3>
          {title} <span>{entries.length}</span>
        </h3>
        <div className="execution-playback">
          <button
            className="icon-button"
            title="Play node from start"
            aria-label="Play node from start"
            disabled={!canReplay}
            onClick={restart}
          >
            <RotateCcw size={14} />
          </button>
          <button
            className="icon-button"
            title={playing ? 'Pause node replay' : 'Play node replay'}
            aria-label={playing ? 'Pause node replay' : 'Play node replay'}
            disabled={!canReplay}
            onClick={play}
          >
            {playing ? <Pause size={14} /> : <Play size={14} />}
          </button>
        </div>
      </div>
      <ol ref={track}>
        {entries.map((entry, index) => {
          const { label, name, description } = entry;
          const Icon =
            entry.state === 'succeeded'
              ? Check
              : entry.state === 'failed'
                ? X
                : entry.state === 'skipped'
                  ? Minus
                  : Circle;
          return (
            <li key={entry.id}>
              <button
                className={`execution-stop state-${entry.state}`}
                ref={entry.id === selected ? active : undefined}
                aria-label={description}
                aria-pressed={entry.id === selected}
                title={description}
                onClick={() => select(entry.id)}
                onKeyDown={(event) => {
                  const next =
                    event.key === 'ArrowRight'
                      ? index + 1
                      : event.key === 'ArrowLeft'
                        ? index - 1
                        : event.key === 'Home'
                          ? 0
                          : event.key === 'End'
                            ? entries.length - 1
                            : -1;
                  if (next < 0 || next >= entries.length) return;
                  event.preventDefault();
                  select(entries[next].id);
                  track.current?.children[next].querySelector('button')?.focus();
                }}
              >
                <span className="execution-dot">
                  <Icon size={12} />
                </span>
                {name && <span className="execution-name">{name}</span>}
                <span className="execution-caption">
                  {label.split(' · ').map((part, index) => (
                    <span key={index}>{part}</span>
                  ))}
                </span>
              </button>
            </li>
          );
        })}
      </ol>
    </section>
  );
}
