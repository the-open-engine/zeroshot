import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowRight, ChevronDown, ChevronRight, Menu, RefreshCw, Search } from 'lucide-react';
import { AppHeader } from './AppHeader';
import { RunHistoryView } from './RunHistoryView';
import { createExampleRunHistory } from './run-history-source';
import { humanize, type RunSummary } from './run-history';
import type { Bootstrap } from './api';
import type { WorkspaceServices } from './workspace-services';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const dateLabel = (time?: number | null) =>
  time
    ? new Date(time).toLocaleString(undefined, {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      })
    : '';
const runState = (run: RunSummary) =>
  run.terminal?.status ?? (run.phase === 'finished' ? 'Finished' : humanize(run.phase));
function mergeRuns(previous: RunSummary[], incoming: RunSummary[]) {
  const byId = new Map(previous.map((run) => [run.runId, run]));
  for (const run of incoming) {
    const old = byId.get(run.runId);
    if (old?.phase === 'finished' && run.phase !== 'finished' && run.historyAvailable) continue;
    byId.set(run.runId, run);
  }
  return [...byId.values()].sort((a, b) => b.runId.localeCompare(a.runId));
}
function selectedRun() {
  const [kind, id] = window.location.hash.slice(1).split('/').slice(1);
  try {
    return id ? { example: kind === 'example', id: decodeURIComponent(id) } : undefined;
  } catch {
    return undefined;
  }
}

/** Standalone navigation owns the run list and hash. Hosts can mount RunHistoryView directly. */
export function StandaloneRuns({
  services,
  bootstrap,
}: {
  services: WorkspaceServices;
  bootstrap: Bootstrap;
}) {
  const history = services.history;
  const exampleSource = useMemo(() => createExampleRunHistory(services.mount), [services.mount]);
  const [selection, setSelection] = useState(selectedRun);
  const [runs, setRuns] = useState<RunSummary[]>([]),
    [examples, setExamples] = useState<RunSummary[]>([]);
  const [listCursor, setListCursor] = useState<string | null>(null),
    [loadingList, setLoadingList] = useState(true);
  const [examplesOpen, setExamplesOpen] = useState(false),
    [query, setQuery] = useState(''),
    [sidebar, setSidebar] = useState(false);
  const [listError, setListError] = useState(''),
    [exampleError, setExampleError] = useState('');
  const listBusy = useRef(false),
    listLoadedMore = useRef(false);
  useEffect(() => {
    const changed = () => setSelection(selectedRun());
    window.addEventListener('hashchange', changed);
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function pollRuns() {
      if (!listBusy.current) {
        listBusy.current = true;
        try {
          const result = await history.list(undefined, controller.signal);
          if (!controller.signal.aborted) {
            setRuns((previous) => mergeRuns(previous, result.runs));
            if (!listLoadedMore.current) setListCursor(result.nextCursor);
            setListError('');
          }
        } catch (error) {
          if (!controller.signal.aborted) setListError(message(error));
        } finally {
          listBusy.current = false;
          if (!controller.signal.aborted) setLoadingList(false);
        }
      }
      if (!controller.signal.aborted) timer = setTimeout(pollRuns, 4000);
    }
    void pollRuns();
    return () => {
      controller.abort();
      clearTimeout(timer);
      window.removeEventListener('hashchange', changed);
    };
  }, [history]);
  async function refreshList(more = false) {
    if (listBusy.current) return;
    listBusy.current = true;
    setLoadingList(true);
    setListError('');
    try {
      const result = await history.list(more ? (listCursor ?? undefined) : undefined);
      setRuns((previous) => mergeRuns(previous, result.runs));
      if (more) listLoadedMore.current = true;
      if (more || !listLoadedMore.current) setListCursor(result.nextCursor);
    } catch (error) {
      setListError(message(error));
    } finally {
      listBusy.current = false;
      setLoadingList(false);
    }
  }
  function choose(run: RunSummary) {
    setSidebar(false);
    window.location.hash = `runs/${run.example ? 'example' : 'local'}/${encodeURIComponent(run.runId)}`;
  }
  async function showExamples() {
    setExamplesOpen((open) => !open);
    if (!examples.length)
      try {
        setExamples((await exampleSource.list()).runs);
        setExampleError('');
      } catch (error) {
        setExampleError(message(error));
      }
  }
  const filtered = runs.filter((run) =>
    `${run.title} ${run.runId}`.toLowerCase().includes(query.toLowerCase())
  );
  const latest = runs.find((run) => run.historyAvailable);
  return (
    <div className="app history-app">
      <AppHeader
        section="runs"
        workspace={bootstrap.workspace}
        navigate={(section) => {
          if (section === 'profiles') window.location.hash = '';
        }}
      >
        <button
          className="icon-button history-sidebar-toggle"
          title="Run list"
          aria-label="Run list"
          onClick={() => setSidebar(!sidebar)}
        >
          <Menu size={18} />
        </button>
      </AppHeader>
      <div className="app-body">
        <nav className={`run-sidebar ${sidebar ? 'visible' : ''}`} aria-label="Runs">
          <div className="sidebar-heading">
            <span className="eyebrow">RUNS</span>
            <button
              className="icon-button"
              title="Refresh runs"
              aria-label="Refresh runs"
              disabled={loadingList}
              onClick={() => void refreshList()}
            >
              <RefreshCw size={15} className={loadingList ? 'spin' : ''} />
            </button>
          </div>
          <label className="run-search">
            <Search size={14} />
            <input
              aria-label="Search runs"
              placeholder="Find a run"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </label>
          <div className="run-list">
            {listError && <p className="history-inline-error">{listError}</p>}
            {!loadingList && !listError && !filtered.length && (
              <p className="run-empty">{query ? 'No matching runs' : 'No runs yet'}</p>
            )}
            {filtered.map((item) => (
              <RunListItem
                key={item.runId}
                run={item}
                active={selection?.id === item.runId && !selection.example}
                choose={choose}
              />
            ))}
            {listCursor && (
              <button
                className="text-button history-more"
                disabled={loadingList}
                onClick={() => void refreshList(true)}
              >
                Load more runs
              </button>
            )}
            <button
              className="examples-toggle"
              aria-expanded={examplesOpen}
              onClick={() => void showExamples()}
            >
              {examplesOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />} Examples{' '}
              <span>10</span>
            </button>
            {examplesOpen && (
              <>
                <p className="example-provenance">Simulated histories</p>
                {exampleError && <p className="history-inline-error">{exampleError}</p>}
                {examples.map((item) => (
                  <RunListItem
                    key={item.runId}
                    run={item}
                    active={selection?.id === item.runId && !!selection.example}
                    choose={choose}
                  />
                ))}
              </>
            )}
          </div>
        </nav>
        <RunHistoryView
          runId={selection?.id}
          source={selection?.example ? exampleSource : history}
          workers={bootstrap.workers ?? []}
          emptyAction={
            latest && (
              <button className="button" onClick={() => choose(latest)}>
                Open latest run <ArrowRight size={15} />
              </button>
            )
          }
        />
      </div>
    </div>
  );
}

function RunListItem({
  run,
  active,
  choose,
}: {
  run: RunSummary;
  active: boolean;
  choose: (run: RunSummary) => void;
}) {
  return (
    <button
      className={`run-list-item ${active ? 'active' : ''}`}
      aria-current={active ? 'true' : undefined}
      onClick={() => choose(run)}
    >
      <span className={`run-dot state-${run.terminal?.status ?? run.phase}`} />
      <span>
        <strong>{run.title}</strong>
        <small>
          {run.example ? 'Example' : dateLabel(run.createdAt) || run.runId.slice(0, 8)}
          <span>{run.historyAvailable ? humanize(runState(run)) : 'History unavailable'}</span>
        </small>
      </span>
    </button>
  );
}
