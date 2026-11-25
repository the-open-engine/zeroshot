import { useEffect, useMemo, useState } from 'react';
import { projectHistory, requestStanding, type RunDetail } from './run-history';

type Inputs = Parameters<typeof projectHistory>;

/** The current time, renewed once when `deadline` passes so what waits on it can say so. */
function useDeadlineClock(deadline: number | undefined) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (deadline === undefined || deadline <= now) return;
    // `now` may be as old as the page: measure from the real time, and catch up at once when
    // the deadline has already passed.
    const remaining = deadline - Date.now();
    if (remaining <= 0) return setNow(Date.now());
    // Timers hold at most about 24.8 days; a later deadline renews the clock and waits again.
    const timer = setTimeout(() => setNow(Date.now()), Math.min(remaining, 2 ** 31 - 1));
    return () => clearTimeout(timer);
  }, [deadline, now]);
  return now;
}

function selectedSuccessor(run: RunDetail | undefined) {
  if (!run?.partView || !run.partId || !run.parts) return;
  const position = run.parts.indexOf(run.partId);
  return position >= 0 ? run.parts[position + 1] : undefined;
}

/**
 * The graph as recorded up to `through`. A suspended request node also says whether it still
 * waits at the selected moment, and its deadline can pass while the page is open.
 */
export function useProjection(
  document: Inputs[0] | undefined,
  events: Inputs[1],
  through: Inputs[2],
  control: Inputs[3],
  run: RunDetail | undefined,
  /** Only the loaded head may use the host's current successor and wall clock. */
  current: boolean
) {
  const projection = useMemo(
    () => document && projectHistory(document, events, through, control),
    [document, events, through, control]
  );
  const successorRunId = current ? selectedSuccessor(run) : undefined;
  const suspension =
    projection?.suspension ?? (current && run?.phase === 'suspended' ? run.suspension : undefined);
  const clock = useDeadlineClock(
    current && !successorRunId ? suspension?.request?.expiresAt : undefined
  );
  const now = current ? clock : projection?.clock.at;
  return useMemo(
    () =>
      projection && {
        ...projection,
        nodes: requestStanding(projection.nodes, suspension, !!successorRunId, now ?? 0),
        successorRunId,
        suspension,
        now,
      },
    [projection, suspension, successorRunId, now]
  );
}
