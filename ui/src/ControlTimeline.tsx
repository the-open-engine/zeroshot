import { ChevronRight } from 'lucide-react';
import { ActivityTimeline } from './ExecutionTimeline';
import { RecordedValue } from './RecordedValue';
import { controlState, controlVisitLabel, type ControlVisit } from './control-history';
import { humanize } from './run-history';

export function ControlTimeline({
  visits,
  selected,
  select,
  reveal,
  playing,
  canReplay,
  play,
  restart,
}: {
  visits: ControlVisit[];
  selected?: string;
  select: (id: string) => void;
  reveal: (node: string) => void;
  playing: boolean;
  canReplay: boolean;
  play: () => void;
  restart: () => void;
}) {
  const current =
    visits.find((visit) => visit.visitId === selected) ??
    visits.reduce<ControlVisit | undefined>(
      (latest, visit) => (!latest || visit.updated >= latest.updated ? visit : latest),
      undefined
    );
  return (
    <>
      <ActivityTimeline
        title="Visits"
        entries={visits.map((visit) => ({
          id: visit.visitId,
          label: controlVisitLabel(visit),
          state: controlState(visit.state),
          description: [
            controlVisitLabel(visit),
            visit.branch ? `Selected ${humanize(visit.branch)}` : humanize(visit.state),
          ].join(' · '),
        }))}
        selected={current?.visitId}
        {...{ select, playing, canReplay, play, restart }}
      />
      {current && (
        <section className="run-inspector-section control-detail">
          {current.branch ? (
            <>
              <h3>Selected path</h3>
              <button className="observed-route" onClick={() => reveal(current.branch!)}>
                {humanize(current.branch)}
                <ChevronRight size={14} />
              </button>
            </>
          ) : (
            <span className={`run-status state-${controlState(current.state)}`}>
              {current.state === 'entered'
                ? 'In progress'
                : current.state === 'succeeded'
                  ? 'Succeeded'
                  : current.state === 'failed'
                    ? 'Failed'
                    : current.state === 'stopped'
                      ? 'Stopped'
                      : 'Completed'}
            </span>
          )}
          {current.detail && <p className="history-muted">{humanize(current.detail)}</p>}
          {current.output != null && (
            <details className="history-value" open>
              <summary>Output</summary>
              <RecordedValue value={current.output} />
            </details>
          )}
        </section>
      )}
    </>
  );
}
