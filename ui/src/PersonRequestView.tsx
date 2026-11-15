import { humanize, type PersonRequest } from './run-history';

const dateTime = new Intl.DateTimeFormat(undefined, {
  month: 'short',
  day: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
});
const relative = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });

export function formatMoment(ms: number) {
  return dateTime.format(new Date(ms));
}

/** A coarse distance ("in 6 days", "3 hours ago") for deadlines a person reads at a glance. */
export function fromNow(ms: number, now = Date.now()) {
  const seconds = (ms - now) / 1000;
  const units: [Intl.RelativeTimeFormatUnit, number][] = [
    ['day', 86_400],
    ['hour', 3_600],
    ['minute', 60],
  ];
  for (const [unit, size] of units)
    if (Math.abs(seconds) >= size) return relative.format(Math.round(seconds / size), unit);
  return relative.format(Math.round(seconds), 'second');
}

/** How long a person took; recorded request and answer times are exact. */
export function formatWait(ms: number) {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return 'under a minute';
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return `${hours} h ${String(minutes % 60).padStart(2, '0')} min`;
  return `${Math.round(hours / 24)} days`;
}

/** One line on where a request stands: answered after a wait, expired, or still waiting. */
export function requestStatus(request: PersonRequest, now = Date.now()) {
  const { resolution, createdAt, expiresAt } = request;
  if (resolution?.kind === 'answer')
    return resolution.at !== undefined && createdAt !== undefined
      ? `Answered after ${formatWait(resolution.at - createdAt)}`
      : 'Answered';
  if (resolution?.kind === 'expiry') return `No answer before ${formatMoment(expiresAt)}`;
  return [
    createdAt !== undefined && `Waiting since ${formatMoment(createdAt)}`,
    expiresAt > now
      ? `expires ${fromNow(expiresAt, now)}`
      : `deadline passed ${formatMoment(expiresAt)}`,
  ]
    .filter(Boolean)
    .join(' · ');
}

/**
 * The question and its allowed answers. Who answered is not recorded, so it is never shown;
 * the chosen label is marked, the others stay quiet. The answer's own output is the
 * execution's output, shown with it.
 */
export function PersonRequestView({
  request,
  runId,
  compact = false,
}: {
  request: PersonRequest;
  /** The paused run an answer continues, for the command hint while waiting. */
  runId?: string;
  compact?: boolean;
}) {
  const answer = request.resolution?.kind === 'answer' ? request.resolution : undefined;
  const fields = Object.entries(request.signals);
  const pending = !request.resolution;
  return (
    <div className={`person-request ${compact ? 'compact' : ''}`}>
      <blockquote>{request.text}</blockquote>
      {fields.length > 0 && (
        <div className="request-options">
          {fields.map(([field, labels]) => (
            <span key={field} className="request-field">
              {fields.length > 1 && <span className="request-field-name">{humanize(field)}</span>}
              {labels.map((label) => {
                const chosen = answer?.signals[field] === label;
                return (
                  <span
                    key={label}
                    className={`request-option ${chosen ? 'chosen' : ''} ${answer && !chosen ? 'passed' : ''}`}
                  >
                    {humanize(label)}
                  </span>
                );
              })}
            </span>
          ))}
        </div>
      )}
      <p className="request-status">{requestStatus(request)}</p>
      {!compact && pending && runId && (
        <p className="request-hint">
          Answer with <code>zeroshot answer {runId} --input answer.json</code>
        </p>
      )}
    </div>
  );
}
