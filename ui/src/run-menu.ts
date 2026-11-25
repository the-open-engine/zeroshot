import { runIdPattern } from './grouped-history';
import { humanize, suspensionStatus, type RunSummary } from './run-history';

export function runMenuStatus(run: RunSummary) {
  if (run.phase === 'queued') return 'Queued';
  if (!run.historyAvailable) return 'History unavailable';
  if (run.phase === 'suspended') return suspensionStatus(run.suspension);
  if (run.phase === 'running' && run.suspendRequested) return 'Suspension requested';
  return humanize(run.terminal?.status ?? run.phase);
}

/** A refreshed interval replaces the physical rows which may have just joined a family.
 * Preserve other pages in the menu, including unavailable or older entries. */
export function mergeRunPage(
  previous: RunSummary[],
  page: { runs: RunSummary[]; nextCursor: string | null },
  after?: string | null
) {
  const byId = new Map(previous.map((run) => [run.runId, run]));
  const refreshed = new Set(page.runs.map((run) => run.runId));
  for (const id of byId.keys()) {
    if (after && id >= after) continue;
    if (page.nextCursor && id < page.nextCursor) continue;
    if (!refreshed.has(id)) byId.delete(id);
  }
  for (const run of page.runs) {
    const old = byId.get(run.runId);
    if (old && old.currentPartId === run.currentPartId && staleStatus(old, run)) continue;
    byId.set(run.runId, run);
  }
  return [...byId.values()].sort((a, b) => b.runId.localeCompare(a.runId));
}

function staleStatus(old: RunSummary, current: RunSummary) {
  if (!current.historyAvailable) return false;
  return old.phase === 'finished'
    ? current.phase !== 'finished'
    : old.phase === 'suspended' && ['admitted', 'running', 'stopping'].includes(current.phase);
}

/** Older parts and runs outside the loaded page can still open through their usual deep link. */
export function searchRunMenu(runs: readonly RunSummary[], value: string) {
  const query = value.trim().toLowerCase();
  const matches = runs.filter((run) =>
    [run.title, run.runId, run.currentPartId].some((field) => field?.toLowerCase().includes(query))
  );
  return { matches, lookupId: !matches.length && runIdPattern.test(query) ? query : undefined };
}
