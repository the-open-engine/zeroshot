import { ChevronRight } from 'lucide-react';
import { RecordedValue } from './RecordedValue';
import { Transcript } from './Transcript';
import { humanize, stateLabel, type Invocation } from './run-history';

/** One execution's state, input, result and transcript, for the inspector and inline details. */
export function InvocationView({
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
export function ValueSection({
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
