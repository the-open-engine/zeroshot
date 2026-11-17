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

/**
 * The graph as recorded up to `through`. A suspended request node also says whether it still
 * waits: its successor may be known only to the host's run list, which refreshes after this
 * view read the run's detail, and its deadline can pass while the page is open.
 */
export function useProjection(
  document: Inputs[0] | undefined,
  events: Inputs[1],
  through: Inputs[2],
  control: Inputs[3],
  run: RunDetail | undefined,
  listedSuccessor: string | undefined
) {
  const projection = useMemo(
    () => document && projectHistory(document, events, through, control),
    [document, events, through, control]
  );
  const successorRunId = run?.workspaceRecovery?.successorRunId ?? listedSuccessor;
  const suspension = run?.phase === 'suspended' ? run.suspension : undefined;
  const now = useDeadlineClock(successorRunId ? undefined : suspension?.request?.expiresAt);
  return useMemo(
    () =>
      projection && {
        ...projection,
        nodes: requestStanding(projection.nodes, suspension, !!successorRunId, now),
        successorRunId,
      },
    [projection, suspension, successorRunId, now]
  );
}
