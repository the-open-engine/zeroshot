import { mergeRunPage, runMenuStatus, searchRunMenu } from './run-menu';
import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowRight, ChevronDown, ChevronRight, RefreshCw, Search } from 'lucide-react';
import { AppHeader } from './AppHeader';
import { RunHistoryBoundary } from './RunHistoryBoundary';
import { workspaceStorageKeys } from './workspace-storage';
import { RunHistoryView } from './RunHistoryView';
import { createExampleRunHistory } from './run-history-source';
import type { RunSummary } from './run-history';
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
function selectedRun() {
  const [kind, id, section, part] = window.location.hash.slice(1).split('/').slice(1);
  try {
    return id && ['local', 'example'].includes(kind)
      ? {
          example: kind === 'example',
          id: decodeURIComponent(id),
          part: section === 'parts' && part ? decodeURIComponent(part) : undefined,
        }
      : undefined;
  } catch {
    return undefined;
  }
}

/** Standalone navigation owns the run list and hash. Hosts can mount RunHistoryView directly. */
type StandaloneRunsProps = { services: WorkspaceServices; bootstrap: Bootstrap; viewScope: string };
export function StandaloneRuns(props: StandaloneRunsProps) {
  return (
    <RunHistoryBoundary workspace={props.bootstrap.workspace}>
      <StandaloneRunsContent {...props} />
    </RunHistoryBoundary>
  );
}
function StandaloneRunsContent({ services, bootstrap, viewScope }: StandaloneRunsProps) {
  const history = services.history;
  const storage = workspaceStorageKeys(services.mount, bootstrap.workspace);
  const exampleSource = useMemo(() => createExampleRunHistory(services.mount), [services.mount]);
  const [selection, setSelection] = useState(selectedRun);
  const source = useMemo(
    () =>
      selection?.example
        ? exampleSource
        : selection?.part && history.forPart
          ? history.forPart(selection.part)
          : history,
    [selection?.example, selection?.part, exampleSource, history]
  );
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
            setRuns((previous) => mergeRunPage(previous, result));
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
      setRuns((previous) => mergeRunPage(previous, result, more ? listCursor : undefined));
      if (more) listLoadedMore.current = true;
      if (more || !listLoadedMore.current) setListCursor(result.nextCursor);
    } catch (error) {
      setListError(message(error));
    } finally {
      listBusy.current = false;
      setLoadingList(false);
    }
  }
  function choose(run: Pick<RunSummary, 'runId' | 'example'>) {
    setSidebar(false);
    window.location.hash = runLink(run.runId, run.example);
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
  const { matches: filtered, lookupId } = searchRunMenu(runs, query);
  const latest = runs.find(
    (run) => run.historyAvailable || (run.phase === 'queued' && (run.partCount ?? 1) > 1)
  );
  // The list's periodic refresh brings the successor a suspended run gains while its page is open.
  const listed = (selection?.example ? examples : runs).find((run) => run.runId === selection?.id);
  return (
    <div className="app history-app">
      <AppHeader
        section="runs"
        workspace={bootstrap.workspace}
        navigate={(section) => {
          if (section === 'profiles') window.location.hash = '';
        }}
        sidebar={{ label: 'Run list', id: 'runs-sidebar', open: sidebar, setOpen: setSidebar }}
      />
      <div className="app-body">
        <nav
          id="runs-sidebar"
          className={`page-sidebar run-sidebar ${sidebar ? 'visible' : ''}`}
          aria-label="Runs"
        >
          <div className="sidebar-heading">
            <span className="sidebar-title">Runs</span>
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
          <form
            className="run-search"
            id="search-runs"
            onSubmit={(event) => {
              event.preventDefault();
              if (lookupId) {
                setQuery('');
                choose({ runId: lookupId });
              } else if (filtered.length === 1) choose(filtered[0]);
            }}
          >
            <Search size={14} />
            <input
              aria-label="Search runs"
              placeholder="Find a run"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </form>
          <div className="run-list">
            {listError && <p className="history-inline-error">{listError}</p>}
            {lookupId ? (
              <button type="submit" form="search-runs" className="text-button history-more">
                Open run by ID <ArrowRight size={14} />
              </button>
            ) : (
              !loadingList &&
              !listError &&
              !filtered.length && (
                <p className="run-empty">{query ? 'No matching runs' : 'No runs yet'}</p>
              )
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
              {examples.length > 0 && <span>{examples.length}</span>}
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
          key={`${selection?.example}:${selection?.id}:${selection?.part ?? ''}`}
          runId={selection?.id}
          viewKey={
            selection &&
            storage.runView(
              `${viewScope}:${selection.example ? 'example' : selection.part ? 'part' : 'history'}`,
              selection.part ?? selection.id
            )
          }
          source={source}
          onOpenRun={(run) => {
            if (selection && run.runId !== selection.id && run.parts?.includes(selection.id)) {
              window.history.replaceState(
                null,
                '',
                runLink(run.runId, selection.example, selection.part)
              );
              setSelection(selectedRun());
            }
          }}
          workers={bootstrap.workers ?? []}
          partHref={
            selection && !selection.example && history.forPart
              ? (part) => runLink(selection.id, false, part)
              : undefined
          }
          listedPartId={listed?.currentPartId}
          listedPhase={listed?.phase}
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
          <span>{runMenuStatus(run)}</span>
        </small>
      </span>
    </button>
  );
}

/** Example runs link to examples; everything else to this workspace's own runs. */
function runLink(id: string, example?: boolean, part?: string): string {
  return `#runs/${example ? 'example' : 'local'}/${encodeURIComponent(id)}${part ? '/parts/' + encodeURIComponent(part) : ''}`;
}
