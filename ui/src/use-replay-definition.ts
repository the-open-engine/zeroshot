import { useEffect, useMemo, useState } from 'react';
import type { RunDetail } from './run-history';
import type { RunHistoryReader } from './run-history-source';
import { readHistoryWhenReady } from './history-readiness';

/** A resume can override the runtime for later parts. Keep a small per-view cache for scrubbing. */
export function useReplayDefinition(
  run: RunDetail | undefined,
  part: string | undefined,
  source: RunHistoryReader
) {
  const root = run?.runId;
  const historical =
    source.forPart && !run?.partView && part !== run?.currentPartId ? part : undefined;
  const cache = useMemo(() => new Map<string, RunDetail>(), [source, root]);
  const [, setRevision] = useState(0);
  const [failure, setFailure] = useState<{ part: string; message: string }>();
  const [retryKey, setRetry] = useState(0);
  useEffect(() => {
    if (!historical || !root || !source.forPart || cache.has(historical)) return;
    if (run?.phase === 'pending' && run.partId === historical) {
      cache.set(historical, run);
      return;
    }
    const controller = new AbortController();
    const exact = source.forPart(historical);
    setFailure(undefined);
    void readHistoryWhenReady(
      () => exact.detail(root, controller.signal),
      controller.signal,
      exact.retryDelay,
      () => {}
    )
      .then((detail) => {
        if (controller.signal.aborted) return;
        cache.set(historical, detail);
        if (cache.size > 6) cache.delete(cache.keys().next().value!);
        setRevision((value) => value + 1);
      })
      .catch((cause) => {
        if (!controller.signal.aborted)
          setFailure({
            part: historical,
            message: cause instanceof Error ? cause.message : String(cause),
          });
      });
    return () => controller.abort();
  }, [historical, root, source, cache, retryKey, run?.partId, run?.phase]);
  const selected = recordedDefinition(run, part, historical, cache);
  const document = useMemo(
    () => selected && { name: selected.runId, graph: selected.graph, runtime: selected.runtime },
    [selected]
  );
  return {
    document,
    error: failure?.part === historical ? failure?.message : undefined,
    loading: Boolean(run && !selected),
    retry: () => setRetry((n) => n + 1),
  };
}

function recordedDefinition(
  run: RunDetail | undefined,
  part: string | undefined,
  historical: string | undefined,
  cache: Map<string, RunDetail>
) {
  if (historical) return cache.get(historical) ?? (historical === run?.partId ? run : undefined);
  if (!run?.partView && run?.partId && part === run.currentPartId && part !== run.partId) return;
  return run;
}
