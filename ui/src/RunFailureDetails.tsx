import type { FailureContext, FailureDiagnostic } from './failure-context';

/** Failure evidence is recorded by the runtime, never inferred from transcript ordering. */
export function RunFailureDetails({
  context,
  openLog,
  hasLog,
}: {
  context: FailureContext;
  openLog: (cursor: string) => void;
  hasLog: (cursor: string) => boolean;
}) {
  const recorded = context.recorded ?? [];
  const diagnostic = (entry: FailureDiagnostic, key: string) => (
    <div className="failure-diagnostic" key={key}>
      {entry.execution && <strong>{entry.execution.node}</strong>}
      <p>{entry.message}</p>
      {entry.cursor &&
        (hasLog(entry.cursor) ? (
          <button className="text-button" onClick={() => openLog(entry.cursor!)}>
            View log · {entry.cursor}
          </button>
        ) : (
          <span className="history-muted">Log not loaded · {entry.cursor}</span>
        ))}
    </div>
  );
  return (
    <section className="run-failure-details" aria-label="Failure details">
      {context.primary && (
        <>
          <h3>Failure</h3>
          {diagnostic(context.primary, 'primary')}
        </>
      )}
      {recorded.length > 0 && (
        <>
          <h3>Recorded failures</h3>
          {recorded.map((entry, index) => diagnostic(entry, `recorded-${index}`))}
        </>
      )}
      {context.incomplete && (
        <p className="history-muted">Additional failure details may be missing. See the run log.</p>
      )}
    </section>
  );
}
