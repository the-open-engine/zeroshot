import { useCallback, useEffect, useRef, useState } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import {
  AlertCircle,
  Braces,
  Check,
  ChevronDown,
  Download,
  FilePlus2,
  Loader2,
  Menu,
  Plus,
  Redo2,
  Undo2,
  Upload,
  X,
} from 'lucide-react';
import { applyWorker } from './workers';
import { RuntimeEditor } from './RuntimeEditor';
import { AuthoringProvider } from './AuthoringProvider';
import { WorkflowCanvas, type WorkflowAction } from './WorkflowCanvas';
import {
  addNext,
  addOutcome,
  appendActivity,
  automaticProtectionTarget,
  parallelSiblingCandidates,
  wrapActivity,
  wrapParallelActivities,
} from './workflow-authoring';
import type { WorkflowEdge } from './workflow-projection';
import { GuardEditor } from './GuardEditor';
import { Field, Inspector } from './Inspector';
import { type Bootstrap, type Summary } from './api';
import type { WorkspaceServices } from './workspace-services';
import { createHostedProfile, useHostWorkspace } from './use-host-workspace';
import { workspaceStorageKeys } from './workspace-storage';
import { acknowledgeProfileSave, snapshotProfileSave } from './profile-save';
import { createNumericDrafts, NumericDraftProvider } from './numeric-drafts';
import {
  addNode,
  blankDocument,
  editableKinds,
  labels,
  replaceBody,
  wrapBody,
  allNodes,
  assertDocument,
  children,
  clone,
  connectAfter,
  findNode,
  isGroup,
  newDocument,
  pathTo,
  removeNode,
  reorder,
  replaceNode,
  type Document,
  type GraphNode,
  type Positions,
} from './domain';
import { AppHeader } from './AppHeader';
import { StandaloneRuns } from './StandaloneRuns';
import { DefaultsPage } from './DefaultsPage';
import { createReviewLoop } from './review-loop';
import {
  flushPendingEdits,
  hasPendingEdits,
  usePendingEdits,
  usePendingText,
} from './pending-edits';
const stringify = (v: unknown) => JSON.stringify(v);
const content = (v: Document): Document => ({ name: v.name, graph: v.graph, runtime: v.runtime });
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));
function loadDraft(key: string) {
  try {
    const saved = JSON.parse(sessionStorage.getItem(key) ?? 'null');
    if (saved) {
      assertDocument(saved.document);
      if (saved.base) assertDocument(saved.base);
      return saved;
    }
  } catch {
    /* A corrupt browser draft must not prevent opening the saved store. */
  }
  return null;
}
export function App({
  services,
  bootstrap,
  host,
  renderRun,
}: {
  services: WorkspaceServices;
  bootstrap: Bootstrap;
  host?: Parameters<typeof useHostWorkspace>[0];
  renderRun?: (runId?: string) => React.ReactNode;
}) {
  const storage = workspaceStorageKeys(services.mount, bootstrap.workspace);
  const [numericDrafts] = useState(createNumericDrafts);
  useEffect(() => () => numericDrafts.clear(), [numericDrafts]);
  const [historyPage, setHistoryPage] = useState(
    () => !host && window.location.hash.startsWith('#runs')
  );
  const [defaultsPage, setDefaultsPage] = useState(
    () => !host && window.location.hash === '#defaults'
  );
  useEffect(() => {
    if (host) return;
    const changed = () => {
      setDefaultsPage(window.location.hash === '#defaults');
      setHistoryPage(window.location.hash.startsWith('#runs'));
    };
    window.addEventListener('hashchange', changed);
    return () => window.removeEventListener('hashchange', changed);
  }, []);
  const [profiles, setProfiles] = useState<Summary[]>([]),
    [doc, setDoc] = useState<Document>(),
    [base, setBase] = useState<Document>(),
    [revision, setRevision] = useState<string>(),
    [selected, setSelected] = useState('');
  const [positions, setPositions] = useState<Positions>({}),
    [inspector, setInspector] = useState(false),
    [sidebar, setSidebar] = useState(false);
  const [inspectorReset, setInspectorReset] = useState(0);
  const [workflowFocus, setWorkflowFocus] = useState<{ name: string; tick: number }>();
  const [workflowAction, setWorkflowAction] = useState<WorkflowAction>();
  const [parallelActivities, setParallelActivities] = useState<string[]>([]);
  const [outcome, setOutcome] = useState<WorkflowEdge>();
  const [past, setPast] = useState<Document[]>([]),
    [future, setFuture] = useState<Document[]>([]),
    [error, setError] = useState(''),
    [notice, setNotice] = useState(''),
    [busy, setBusy] = useState(false),
    [loading, setLoading] = useState(true);
  const [validation, setValidation] = useState<{
      state: 'checking' | 'valid' | 'invalid';
      message?: string;
    }>({ state: 'checking' }),
    [issues, setIssues] = useState(false);
  const [modal, setModal] = useState<
      | 'new'
      | 'save'
      | 'import'
      | 'json'
      | 'body'
      | 'otherwise'
      | 'runtime'
      | 'workflow-action'
      | 'outcome'
      | null
    >(null),
    [name, setName] = useState(''),
    [template, setTemplate] = useState('blank'),
    [json, setJson] = useState(''),
    [jsonInitial, setJsonInitial] = useState(''),
    [jsonTarget, setJsonTarget] = useState(''),
    [modalError, setModalError] = useState(''),
    [copy, setCopy] = useState(false);
  const [bodyParent, setBodyParent] = useState(''),
    [bodyKind, setBodyKind] = useState('step');
  const [confirm, setConfirm] = useState<{
    title: string;
    body: string;
    action: () => void;
  } | null>(null);
  const current = useRef(doc);
  const currentRevision = useRef(revision);
  currentRevision.current = revision;
  const documentGeneration = useRef(0);
  const savePending = useRef(false);
  current.current = doc;
  const saveAction = useRef<() => void>(() => {});
  saveAction.current = () => {
    void saveDialog();
  };
  const editTime = useRef({ key: '', time: 0 });
  const dirty = !!doc && (!base || stringify(doc) !== stringify(base));
  useEffect(() => {
    if (!historyPage) window.document.title = `${doc?.name || 'Profiles'} · Zeroshot`;
  }, [historyPage, doc?.name]);
  useEffect(() => {
    if (base?.name) {
      try {
        sessionStorage.setItem(storage.lastProfile, base.name);
      } catch {
        /* optional preference */
      }
    }
  }, [base?.name, storage.lastProfile]);
  const pendingEdits = usePendingEdits();
  const jsonDirty = modal === 'json' && json !== jsonInitial;
  usePendingText(jsonDirty);
  const refresh = () => services.profiles.list().then((r) => setProfiles(r.profiles));
  const positionKey = doc?.name ? storage.layout(doc.name) : '';
  const apply = useCallback((next: Document, key = '') => {
    try {
      assertDocument(next);
    } catch (e) {
      setError(message(e));
      return;
    }
    setDoc((previous) => {
      if (!previous) return next;
      if (stringify(previous) === stringify(next)) return previous;
      const now = Date.now();
      if (!key || key !== editTime.current.key || now - editTime.current.time > 900)
        setPast((p) => [...p.slice(-59), clone(previous)]);
      editTime.current = { key, time: now };
      setFuture([]);
      return next;
    });
    setError('');
    setNotice('');
  }, []);
  function adopt(next: Document, saved?: Document, rev?: string, layout?: Positions) {
    numericDrafts.clear();
    documentGeneration.current++;
    current.current = content(next);
    currentRevision.current = rev;
    setDoc(current.current);
    setBase(saved ? content(saved) : undefined);
    setRevision(rev);
    setPast([]);
    setFuture([]);
    setWorkflowFocus(undefined);
    setInspector(false);
    setSelected(children(next.graph.root)[0]?.name ?? next.graph.root.name);
    setError('');
    setNotice('');
    setIssues(false);
    setSidebar(false);
    let stored = layout ?? {};
    if (!layout && next.name)
      try {
        stored = JSON.parse(localStorage.getItem(storage.layout(next.name)) ?? '{}');
      } catch {
        /* optional layout */
      }
    setPositions(stored);
    editTime.current = { key: '', time: 0 };
  }
  const hosted = useHostWorkspace(host, {
    get document() {
      return current.current;
    },
    get revision() {
      return currentRevision.current;
    },
    get generation() {
      return documentGeneration.current;
    },
    dirty,
    pending: pendingEdits || jsonDirty,
    busy,
    loading,
    validation,
    createProfile: (templateId) => createHostedProfile(bootstrap.templates, templateId),
    adopt,
    acknowledge: acceptSaved,
    discard: () => {
      setModal(null);
      if (base) adopt(base, base, revision);
      else {
        numericDrafts.clear();
        documentGeneration.current++;
        current.current = undefined;
        setDoc(undefined);
      }
      try {
        sessionStorage.removeItem(storage.draft);
      } catch {
        /* optional recovery */
      }
    },
  });
  function acceptSaved(acknowledged: NonNullable<ReturnType<typeof acknowledgeProfileSave>>) {
    current.current = acknowledged.document;
    currentRevision.current = acknowledged.revision;
    setDoc(acknowledged.document);
    setBase(acknowledged.base);
    setRevision(acknowledged.revision);
    setModal((active) => (active === 'save' ? null : active));
    setNotice(
      !acknowledged.changed && !hasPendingEdits()
        ? 'Profile saved'
        : 'Earlier changes saved. Newer edits are unsaved.'
    );
  }
  useEffect(() => {
    if (host) {
      const draft = loadDraft(storage.draft);
      if (draft) {
        adopt(draft.document, draft.base, draft.revision, draft.positions);
        hosted.restore(draft.documentId);
        setNotice('Draft restored');
      }
      setLoading(false);
      return;
    }
    let live = true;
    const controller = new AbortController();
    services.profiles
      .list(controller.signal)
      .then(async (list) => {
        if (!live) return;
        setProfiles(list.profiles);
        const draft = loadDraft(storage.draft);
        if (draft) {
          adopt(draft.document, draft.base, draft.revision, draft.positions);
          setNotice('Draft restored');
        } else if (list.profiles.length) {
          let last: string | null = null;
          try {
            last = sessionStorage.getItem(storage.lastProfile);
          } catch {
            /* optional preference */
          }
          const profile =
            list.profiles.find((profile) => profile.name === last) ?? list.profiles[0];
          const result = await services.profiles.load(profile.name, controller.signal);
          if (live) adopt(result.profile, result.profile, result.revision);
        } else if (bootstrap.templates[0]) adopt(newDocument(bootstrap.templates[0]));
      })
      .catch((e) => {
        if (live) setError(message(e));
      })
      .finally(() => {
        if (live) setLoading(false);
      });
    return () => {
      live = false;
      controller.abort();
    };
  }, []);
  useEffect(() => {
    if (!doc) return;
    try {
      if (dirty)
        sessionStorage.setItem(
          storage.draft,
          stringify({
            document: doc,
            base,
            revision,
            positions,
            documentId: hosted.profileDocumentId,
          })
        );
      else sessionStorage.removeItem(storage.draft);
    } catch {
      setNotice('Draft recovery is unavailable. Save your changes before closing.');
    }
  }, [doc, base, revision, positions, dirty, storage.draft]);
  useEffect(() => {
    if (positionKey)
      try {
        localStorage.setItem(positionKey, stringify(positions));
      } catch {
        /* Layout preferences are optional; profile data stays in the native store. */
      }
  }, [positions, positionKey]);
  useEffect(() => {
    if (!doc) return;
    const controller = new AbortController();
    setValidation({ state: 'checking' });
    const snapshot = { graph: doc.graph, runtime: doc.runtime };
    const timer = setTimeout(() => {
      services.authoring
        .validate(snapshot, controller.signal)
        .then(() => {
          if (!controller.signal.aborted) setValidation({ state: 'valid' });
        })
        .catch((e) => {
          if (!controller.signal.aborted) setValidation({ state: 'invalid', message: message(e) });
        });
    }, 450);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [doc?.graph, doc?.runtime]);
  useEffect(() => {
    const guard = (e: BeforeUnloadEvent) => {
      if (dirty || jsonDirty || hasPendingEdits()) {
        e.preventDefault();
        e.returnValue = '';
      }
    };
    window.addEventListener('beforeunload', guard);
    return () => window.removeEventListener('beforeunload', guard);
  }, [dirty, jsonDirty]);
  useEffect(() => {
    if (!doc) return;
    numericDrafts.prune(doc.graph.root);
    if (!findNode(doc.graph.root, selected)) setSelected(doc.graph.root.name);
  }, [doc, selected, numericDrafts]);
  function undo() {
    if (!doc || !past.length || busy || loading) return;
    setFuture((f) => [clone(doc), ...f]);
    setDoc(past.at(-1));
    setPast((p) => p.slice(0, -1));
    editTime.current = { key: '', time: 0 };
  }
  function redo() {
    if (!doc || !future.length || busy || loading) return;
    setPast((p) => [...p, clone(doc)]);
    setDoc(future[0]);
    setFuture((f) => f.slice(1));
    editTime.current = { key: '', time: 0 };
  }
  async function saveDialog(asCopy = false) {
    if (host) {
      hosted.requestSave();
      return;
    }
    if (!doc || busy || loading) return;
    const generation = documentGeneration.current;
    try {
      await flushPendingEdits();
    } catch (cause) {
      setError(message(cause));
      return;
    }
    if (generation !== documentGeneration.current || !current.current) return;
    const latest = current.current;
    setCopy(asCopy);
    setName(asCopy ? (latest.name ? `${latest.name}-copy` : '') : latest.name);
    setModalError('');
    if (latest.name && base && !asCopy) void save(latest.name, false);
    else setModal('save');
  }
  useEffect(() => {
    function shortcut(e: KeyboardEvent) {
      if (modal || confirm || defaultsPage || historyPage || hosted.showingRun) return;
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
        e.preventDefault();
        saveAction.current();
        return;
      }
      const el = e.target as HTMLElement;
      if (el.closest('input,textarea,select,[contenteditable=true]')) return;
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z') {
        e.preventDefault();
        e.shiftKey ? redo() : undo();
      }
    }
    window.addEventListener('keydown', shortcut);
    return () => window.removeEventListener('keydown', shortcut);
  });
  function guard(action: () => void) {
    if (busy || loading) return;
    if (dirty || hasPendingEdits())
      setConfirm({
        title: 'Discard unsaved changes?',
        body: 'Your saved profile will stay unchanged.',
        action,
      });
    else action();
  }
  async function load(name: string) {
    const openingFrom = current.current;
    setLoading(true);
    try {
      const result = await services.profiles.load(name);
      if (current.current !== openingFrom) {
        setError(
          'The current profile changed while loading. Open the saved profile again when ready.'
        );
        return;
      }
      adopt(result.profile, result.profile, result.revision);
    } catch (e) {
      setError(message(e));
    } finally {
      setLoading(false);
    }
  }
  async function save(profileName: string, asCopy: boolean) {
    if (!doc || busy || loading || savePending.current) return;
    savePending.current = true;
    const generation = documentGeneration.current;
    const focusBeforeSave = document.activeElement as HTMLElement | null;
    setBusy(true);
    setModalError('');
    setError('');
    try {
      await flushPendingEdits();
      if (generation !== documentGeneration.current || !current.current) return;
      const snapshot = snapshotProfileSave(current.current, {
        name: profileName,
        revision: asCopy ? undefined : revision,
        generation,
        pending: hasPendingEdits() || jsonDirty,
      });
      const result = await services.profiles.save(snapshot.request);
      if (!current.current) return;
      const acknowledged = acknowledgeProfileSave(
        snapshot,
        {
          generation: documentGeneration.current,
          document: current.current,
        },
        result
      );
      if (!acknowledged) return;
      acceptSaved(acknowledged);
      await refresh().catch(() => setNotice('Profile saved. Reload to refresh the profile list.'));
    } catch (e) {
      setModalError(message(e));
      setError(message(e));
      setIssues(true);
    } finally {
      savePending.current = false;
      setBusy(false);
      requestAnimationFrame(() => {
        if (focusBeforeSave?.isConnected && document.activeElement === document.body)
          focusBeforeSave.focus();
      });
    }
  }
  function openJson(target: string) {
    if (!doc || loading) return;
    const value =
      target === 'graph'
        ? doc.graph
        : target === 'runtime'
          ? doc.runtime
          : findNode(doc.graph.root, target);
    const text = JSON.stringify(value, null, 2);
    setJson(text);
    setJsonInitial(text);
    setJsonTarget(target);
    setModalError('');
    setModal('json');
  }
  function closeModal() {
    if (busy) return;
    if (jsonDirty)
      setConfirm({
        title: 'Discard JSON changes?',
        body: 'Changes in this JSON editor have not been applied.',
        action: () => setModal(null),
      });
    else setModal(null);
  }
  function applyJson() {
    if (!doc) return;
    try {
      let next = clone(doc);
      const value = JSON.parse(json);
      if (jsonTarget === 'graph') {
        if (!value || typeof value !== 'object' || Array.isArray(value) || !value.root)
          throw new Error('Graph JSON must be an object containing a root node.');
        next.graph = value;
      } else if (jsonTarget === 'runtime') {
        if (!value || typeof value !== 'object' || Array.isArray(value) || !value.nodes)
          throw new Error('Runtime JSON must be an object containing node bindings.');
        next.runtime = value;
      } else {
        next = replaceNode(doc, jsonTarget, value);
        setSelected(value.name);
      }
      assertDocument(next);
      if (jsonTarget === 'graph') numericDrafts.clear();
      else if (jsonTarget !== 'runtime') {
        const replaced = findNode(doc.graph.root, jsonTarget);
        if (replaced) allNodes(replaced).forEach((node) => numericDrafts.clear(node.name));
      }
      apply(next);
      if (jsonTarget === 'graph' || jsonTarget === selected)
        setInspectorReset((value) => value + 1);
      setModal(null);
    } catch (e) {
      setModalError(message(e));
    }
  }
  function importJson() {
    try {
      const value = JSON.parse(json);
      assertDocument(value);
      const next = { name: value.name ?? '', graph: value.graph, runtime: value.runtime };
      adopt(next);
      setModal(null);
      setNotice('Imported as an unsaved profile');
    } catch (e) {
      setModalError(message(e));
    }
  }
  function editNode(node: GraphNode) {
    if (!doc) return;
    const next = replaceNode(doc, selected, node);
    apply(next, `node.${selected}`);
    if (selected !== node.name) {
      setSelected(node.name);
    }
  }
  function attempt(action: () => void) {
    try {
      action();
    } catch (e) {
      setError(message(e));
    }
  }
  function select(name: string) {
    setSelected(name);
    setInspector(true);
  }
  function reveal(name: string) {
    select(name);
    setWorkflowFocus({ name, tick: Date.now() });
  }
  function beginWorkflowAction(action: WorkflowAction) {
    setWorkflowAction(action);
    setParallelActivities([]);
    setBodyKind('step');
    setModalError('');
    setModal('workflow-action');
  }
  async function applyWorkflowAction() {
    if (!doc || !workflowAction || busy) return;
    try {
      const { kind, owner } = workflowAction;
      const worker = bodyKind.startsWith('worker:')
        ? bootstrap?.workers?.find((w) => w.id === bodyKind.slice(7))
        : undefined;
      if (bodyKind.startsWith('worker:') && !worker)
        throw new Error('Reload the native worker choices.');
      const type = worker ? 'verifier' : bodyKind;
      const edit =
        kind === 'review'
          ? createReviewLoop(doc, owner)
          : kind === 'parallel'
            ? wrapParallelActivities(doc, owner, parallelActivities)
            : kind === 'repeat'
              ? wrapActivity(doc, owner, 'loop')
              : kind === 'next'
                ? addNext(doc, owner, type)
                : kind === 'outcome'
                  ? addOutcome(doc, owner, type)
                  : appendActivity(doc, owner, type);
      let next =
        worker && !['parallel', 'repeat', 'review'].includes(kind)
          ? applyWorker(edit.document, edit.name, worker, bootstrap?.workers)
          : edit.document;
      // Rust authors ordinary completion and safe failure handling as explicit native nodes.
      const nodes = allNodes(next.graph.root);
      const structurallyComplete = nodes.every(
        (node) => !isGroup(node) || children(node).length > 0
      );
      if (
        !base &&
        next.graph.root.kind === 'seq' &&
        structurallyComplete &&
        !nodes.some((node) => ['succeed', 'fail'].includes(node.kind))
      ) {
        setBusy(true);
        next = await services.authoring.outcome(next, {
          kind: 'complete',
          owner: next.graph.root.name,
        });
      }
      if (
        kind !== 'review' &&
        (['step', 'verifier'].includes(type) || kind === 'parallel' || kind === 'repeat')
      ) {
        const protectionTarget = automaticProtectionTarget(next, edit.name);
        if (protectionTarget) {
          setBusy(true);
          next = await services.authoring.outcome(next, {
            kind: 'protect',
            node: protectionTarget,
          });
        }
      }
      if (current.current !== doc) throw new Error('The profile changed. Add the activity again.');
      apply(next);
      reveal(edit.name);
      setModal(null);
    } catch (cause) {
      setModalError(message(cause));
    } finally {
      setBusy(false);
    }
  }
  const node = doc ? findNode(doc.graph.root, selected) : undefined;
  const nodePath = doc && node ? pathTo(doc.graph.root, node.name) : [];
  const parent = nodePath.at(-2);
  function bodyDialog(name: string, kind = 'step') {
    if (loading) return;
    setBodyParent(name);
    setBodyKind(kind);
    setModalError('');
    setModal('body');
  }
  function wrap(name: string) {
    if (!doc || loading) return;
    attempt(() => {
      const next = wrapBody(doc, name);
      apply(next.document);
      reveal(next.name);
    });
  }
  function exportDocument() {
    if (!doc) return;
    const blob = new Blob(
      [JSON.stringify({ name: doc.name, graph: doc.graph, runtime: doc.runtime }, null, 2) + '\n'],
      { type: 'application/json' }
    );
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `${doc.name || 'profile'}.json`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  return (
    <NumericDraftProvider value={numericDrafts}>
      <AuthoringProvider value={services.authoring}>
        <div
          className={`app ${host ? 'embedded-app' : ''}`}
          hidden={defaultsPage || historyPage || hosted.showingRun}
        >
          {!host && (
            <AppHeader
              section="profiles"
              workspace={bootstrap.workspace}
              navigate={(section) => {
                if (section === 'runs') window.location.hash = 'runs';
              }}
            >
              <button
                className="icon-button sidebar-toggle"
                title="Profiles"
                aria-label="Profiles"
                onClick={() => setSidebar(!sidebar)}
              >
                <Menu size={19} />
              </button>
            </AppHeader>
          )}
          <div className="app-body">
            {!host && (
              <nav className={`profile-sidebar ${sidebar ? 'visible' : ''}`} aria-label="Profiles">
                <div className="sidebar-heading">
                  <span className="eyebrow">PROFILES</span>
                  <button
                    className="icon-button"
                    title="New profile"
                    aria-label="New profile"
                    disabled={busy || loading}
                    onClick={() =>
                      guard(() => {
                        setName('');
                        setModalError('');
                        setModal('new');
                      })
                    }
                  >
                    <Plus size={17} />
                  </button>
                </div>
                <div className="profile-list">
                  {profiles.map((p) => (
                    <button
                      key={p.id}
                      disabled={busy || loading}
                      className={`profile-item ${doc?.name === p.name && !!base ? 'active' : ''}`}
                      onClick={() => guard(() => void load(p.name))}
                    >
                      <Braces size={16} />
                      <span>{p.name}</span>
                      {p.isDefault && (
                        <span className="default-mark" title="Default profile">
                          •
                        </span>
                      )}
                    </button>
                  ))}
                  {!profiles.length && !loading && (
                    <div className="no-profiles">No saved profiles</div>
                  )}
                  {doc && !base && (
                    <div className="draft-item">
                      <FilePlus2 size={16} />
                      <span>{doc.name || 'Untitled profile'}</span>
                      <span className="draft-dot" title="Unsaved" />
                    </div>
                  )}
                </div>
                <button
                  className="text-button import-button"
                  disabled={busy || loading}
                  onClick={() =>
                    guard(() => {
                      setJson('');
                      setModalError('');
                      setModal('import');
                    })
                  }
                >
                  <Upload size={16} /> Import JSON
                </button>
              </nav>
            )}
            <main className="workspace" inert={loading} aria-busy={loading}>
              {doc ? (
                <>
                  <div className="workspace-header">
                    <div className="profile-title">
                      <h1>{doc.name || 'Untitled profile'}</h1>
                      {(dirty || pendingEdits) && (
                        <span className="unsaved" title="Unsaved changes">
                          Unsaved
                        </span>
                      )}
                    </div>
                    <div className="header-actions">
                      <div className="history-actions">
                        <button
                          className="icon-button"
                          aria-label="Undo"
                          title="Undo"
                          disabled={!past.length || busy}
                          onClick={undo}
                        >
                          <Undo2 size={17} />
                        </button>
                        <button
                          className="icon-button"
                          aria-label="Redo"
                          title="Redo"
                          disabled={!future.length || busy}
                          onClick={redo}
                        >
                          <Redo2 size={17} />
                        </button>
                      </div>
                      <button
                        className="icon-button"
                        aria-label="Export JSON"
                        title="Export JSON"
                        onClick={exportDocument}
                      >
                        <Download size={18} />
                      </button>
                      {!host && (
                        <button
                          className="button save-copy"
                          disabled={busy}
                          onClick={() => saveDialog(true)}
                        >
                          Save as
                        </button>
                      )}
                      {!host && (
                        <button
                          className="button primary"
                          disabled={busy || (!dirty && !pendingEdits && !!base)}
                          onClick={() => saveDialog()}
                        >
                          {busy ? <Loader2 className="spin" size={16} /> : <Check size={16} />}{' '}
                          {busy ? 'Saving' : 'Save profile'}
                        </button>
                      )}
                    </div>
                  </div>
                  <div className={`editor-body ${inspector ? '' : 'inspector-closed'}`}>
                    <ReactFlowProvider>
                      <WorkflowCanvas
                        document={doc}
                        selected={selected}
                        positions={positions}
                        setPositions={setPositions}
                        workers={bootstrap?.workers ?? []}
                        select={reveal}
                        focus={workflowFocus}
                        action={beginWorkflowAction}
                        editOutcome={(edge) => {
                          setOutcome(edge);
                          setModal('outcome');
                        }}
                        reorder={(source, target) =>
                          attempt(() => {
                            const sourceParent = pathTo(doc.graph.root, source).at(-2);
                            const targetParent = pathTo(doc.graph.root, target).at(-2);
                            if (
                              sourceParent?.kind !== 'seq' ||
                              sourceParent.name !== targetParent?.name
                            )
                              throw new Error(
                                'These activities have different execution scopes. Use Add next, Run in parallel, or Repeat to preserve their flow.'
                              );
                            apply(connectAfter(doc, sourceParent.name, source, target));
                          })
                        }
                        runtime={() => setModal('runtime')}
                        json={() => openJson('graph')}
                        info={() => {
                          if (host) hosted.navigate('defaults');
                          else window.location.hash = 'defaults';
                        }}
                        inspectorHidden={!inspector}
                        showInspector={() => setInspector(true)}
                      />
                    </ReactFlowProvider>
                    {inspector && (
                      <Inspector
                        key={`${documentGeneration.current}:${inspectorReset}`}
                        document={doc}
                        node={node}
                        parent={parent}
                        edit={apply}
                        updateNode={editNode}
                        openJson={openJson}
                        openRuntime={() => setModal('runtime')}
                        move={(offset) =>
                          attempt(() => {
                            if (parent && node) apply(reorder(doc, parent.name, node.name, offset));
                          })
                        }
                        remove={() =>
                          setConfirm({
                            title: `Remove ${node?.name}?`,
                            body: isGroup(node!)
                              ? 'This removes the group and all its nodes. References elsewhere may need updating.'
                              : 'References to this node may need updating.',
                            action: () =>
                              attempt(() => {
                                if (node && parent) {
                                  apply(removeNode(doc, parent.name, node.name));
                                  setSelected(parent.name);
                                }
                              }),
                          })
                        }
                        close={() => setInspector(false)}
                        schema={bootstrap?.runtimeSchema}
                        workers={bootstrap?.workers ?? []}
                        replaceBody={bodyDialog}
                        wrapBody={wrap}
                        selectNode={reveal}
                        addOtherwise={(name) => {
                          setBodyParent(name);
                          setBodyKind('step');
                          setModalError('');
                          setModal('otherwise');
                        }}
                      />
                    )}
                  </div>
                  <footer className="status-bar">
                    <button
                      className={`validation ${validation.state}`}
                      onClick={() => setIssues(!issues)}
                      aria-expanded={issues}
                    >
                      {validation.state === 'checking' ? (
                        <Loader2 size={14} className="spin" />
                      ) : validation.state === 'valid' ? (
                        <Check size={14} />
                      ) : (
                        <AlertCircle size={14} />
                      )}
                      <span>
                        {validation.state === 'checking'
                          ? 'Checking profile'
                          : validation.state === 'valid'
                            ? 'Profile valid'
                            : 'Needs attention'}
                      </span>
                      {validation.state === 'invalid' && <ChevronDown size={13} />}
                    </button>
                    <span className="status-message" role="status">
                      {notice}
                    </span>
                    <span className="status-count">
                      {
                        allNodes(doc.graph.root).filter((node) =>
                          ['step', 'verifier'].includes(node.kind)
                        ).length
                      }{' '}
                      activities
                    </span>
                  </footer>
                  {issues && validation.message && (
                    <div className="validation-detail" role="alert">
                      <AlertCircle size={17} />
                      <span>{validation.message}</span>
                      <button
                        className="icon-button"
                        aria-label="Dismiss validation details"
                        onClick={() => setIssues(false)}
                      >
                        <X size={16} />
                      </button>
                    </div>
                  )}
                </>
              ) : (
                <div className="loading-state">
                  {loading ? (
                    <>
                      <Loader2 className="spin" /> Loading profiles
                    </>
                  ) : (
                    <>
                      <AlertCircle /> Could not open profiles.
                      <button className="button" onClick={() => window.location.reload()}>
                        Retry
                      </button>
                    </>
                  )}
                </div>
              )}
              {error && !(issues && error === validation.message) && (
                <div className="error-banner" role="alert">
                  <AlertCircle size={17} />
                  <span>{error}</span>
                  <button
                    className="icon-button"
                    aria-label="Dismiss error"
                    onClick={() => setError('')}
                  >
                    <X size={16} />
                  </button>
                </div>
              )}
            </main>
          </div>
          {modal && (
            <Modal
              title={
                modal === 'workflow-action'
                  ? workflowAction?.kind === 'parallel'
                    ? 'Run in parallel'
                    : workflowAction?.kind === 'review'
                      ? 'Repeat until approved'
                      : workflowAction?.kind === 'repeat'
                        ? 'Repeat activity'
                        : workflowAction?.kind === 'outcome'
                          ? 'Add outcome'
                          : workflowAction?.kind === 'next'
                            ? 'Add next activity'
                            : 'Add activity'
                  : modal === 'outcome'
                    ? 'Outcome condition'
                    : modal === 'runtime'
                      ? 'Runtime'
                      : modal === 'otherwise'
                        ? 'Add otherwise branch'
                        : modal === 'body'
                          ? 'Replace body'
                          : modal === 'new'
                            ? 'New profile'
                            : modal === 'save'
                              ? copy
                                ? 'Save a copy'
                                : 'Save profile'
                              : modal === 'import'
                                ? 'Import profile'
                                : `${jsonTarget === 'graph' ? 'Graph' : jsonTarget === 'runtime' ? 'Runtime' : jsonTarget} JSON`
              }
              close={closeModal}
              wide={modal === 'json' || modal === 'import'}
            >
              {modal === 'workflow-action' && workflowAction && (
                <form
                  onSubmit={(event) => {
                    event.preventDefault();
                    applyWorkflowAction();
                  }}
                >
                  {!['parallel', 'repeat', 'review'].includes(workflowAction.kind) && (
                    <Field label="Activity type">
                      <select
                        value={bodyKind}
                        onChange={(event) => setBodyKind(event.target.value)}
                      >
                        {editableKinds
                          .filter((kind) => kind !== 'seq')
                          .map((kind) => (
                            <option key={kind} value={kind}>
                              {kind === 'choice'
                                ? 'Decision'
                                : kind === 'step'
                                  ? 'Agent'
                                  : kind === 'succeed'
                                    ? 'Finish early'
                                    : kind === 'fail'
                                      ? 'Stop outcome'
                                      : kind === 'loop'
                                        ? 'Repeat a set number of times'
                                        : kind === 'map'
                                          ? 'For each item'
                                          : labels[kind]}
                            </option>
                          ))}
                        {bootstrap?.workers
                          ?.filter((w) => w.runtimeKind === 'git_delivery')
                          .map((w) => (
                            <option key={w.id} value={`worker:${w.id}`}>
                              {w.label}
                            </option>
                          ))}
                      </select>
                    </Field>
                  )}
                  {workflowAction.kind === 'parallel' &&
                    doc &&
                    (() => {
                      const candidates = parallelSiblingCandidates(doc, workflowAction.owner);
                      if (candidates.length < 2) return null;
                      const owner = candidates.findIndex(
                        (node) => node.name === workflowAction.owner
                      );
                      return (
                        <fieldset className="control-labels">
                          <legend>Activities</legend>
                          {candidates.map((node, index) => (
                            <label key={node.name}>
                              <input
                                type="checkbox"
                                aria-label={`Run ${node.name} in parallel`}
                                checked={index === owner || parallelActivities.includes(node.name)}
                                disabled={index === owner}
                                onChange={(event) => {
                                  const checked = event.target.checked;
                                  setParallelActivities((current) =>
                                    candidates
                                      .filter(
                                        (candidate, at) =>
                                          at !== owner &&
                                          (checked
                                            ? current.includes(candidate.name) ||
                                              (at >= Math.min(owner, index) &&
                                                at <= Math.max(owner, index))
                                            : current.includes(candidate.name) &&
                                              (index < owner ? at > index : at < index))
                                      )
                                      .map((candidate) => candidate.name)
                                  );
                                  setModalError('');
                                }}
                              />
                              {node.name}
                            </label>
                          ))}
                        </fieldset>
                      );
                    })()}
                  {modalError && (
                    <p className="modal-error" role="alert">
                      {modalError}
                    </p>
                  )}
                  <div className="modal-actions">
                    <button type="button" className="button" onClick={closeModal}>
                      Cancel
                    </button>
                    <button className="button primary" type="submit" disabled={busy}>
                      {workflowAction.kind === 'review'
                        ? 'Add reviewer and repeat'
                        : workflowAction.kind === 'repeat'
                          ? 'Create loop'
                          : workflowAction.kind === 'parallel'
                            ? 'Create parallel group'
                            : 'Add activity'}
                    </button>
                  </div>
                </form>
              )}
              {modal === 'outcome' &&
                doc &&
                outcome?.owner &&
                (() => {
                  const choice = findNode(doc.graph.root, outcome.owner!);
                  if (choice?.kind !== 'choice')
                    return <p>This path no longer belongs to a decision.</p>;
                  const branch =
                    outcome.branchIndex !== undefined
                      ? choice.branches[outcome.branchIndex]
                      : undefined;
                  return (
                    <>
                      {branch && (
                        <GuardEditor
                          key={`${choice.name}-${outcome.branchIndex}`}
                          document={doc}
                          node={choice}
                          value={branch.when}
                          onChange={(when) =>
                            apply(
                              replaceNode(doc, choice.name, {
                                ...choice,
                                branches: choice.branches.map((b: any, i: number) =>
                                  i === outcome.branchIndex ? { ...b, when } : b
                                ),
                              }),
                              `outcome.${choice.name}.${outcome.branchIndex}`
                            )
                          }
                        />
                      )}
                      <div className="modal-actions">
                        <button
                          className="button"
                          onClick={() => {
                            setModal(null);
                            reveal(choice.name);
                          }}
                        >
                          Show decision
                        </button>
                        <button className="button primary" onClick={() => setModal(null)}>
                          Done
                        </button>
                      </div>
                    </>
                  );
                })()}
              {modal === 'runtime' && doc && (
                <RuntimeEditor
                  document={doc}
                  schema={bootstrap?.runtimeSchema}
                  edit={apply}
                  openJson={() => openJson('runtime')}
                />
              )}
              {(modal === 'body' || modal === 'otherwise') && (
                <form
                  onSubmit={(e) => {
                    e.preventDefault();
                    if (!doc) return;
                    try {
                      if (modal === 'otherwise') {
                        const next = addNode(doc, bodyParent, bodyKind, undefined, true);
                        apply(next.document);
                        reveal(next.name);
                      } else {
                        const next = replaceBody(doc, bodyParent, bodyKind);
                        assertDocument(next);
                        apply(next);
                        const body = findNode(next.graph.root, bodyParent)!.body;
                        reveal(body.name);
                      }
                      setModal(null);
                    } catch (e) {
                      setModalError(message(e));
                    }
                  }}
                >
                  {modal === 'body' && (
                    <p className="body-description">
                      Replace the current body of <strong>{bodyParent}</strong>. Its nodes and
                      runtime settings will be removed. References elsewhere may need updating.
                    </p>
                  )}
                  <Field label={modal === 'body' ? 'New body' : 'Node type'}>
                    <select value={bodyKind} onChange={(e) => setBodyKind(e.target.value)}>
                      {editableKinds.map((kind) => (
                        <option key={kind} value={kind}>
                          {labels[kind]}
                        </option>
                      ))}
                    </select>
                  </Field>
                  {modalError && (
                    <p className="modal-error" role="alert">
                      {modalError}
                    </p>
                  )}
                  <div className="modal-actions">
                    <button type="button" className="button" onClick={closeModal}>
                      Cancel
                    </button>
                    <button type="submit" className="button primary">
                      {modal === 'body' ? 'Replace body' : 'Add otherwise branch'}
                    </button>
                  </div>
                </form>
              )}
              {modal === 'new' && (
                <form
                  onSubmit={(e) => {
                    e.preventDefault();
                    const chosen =
                      template === 'blank'
                        ? bootstrap?.templates[0]
                        : bootstrap?.templates.find((t) => (t.id ?? t.name) === template);
                    if (chosen) {
                      adopt({
                        ...(template === 'blank' ? blankDocument(chosen) : newDocument(chosen)),
                        name,
                      });
                      setModal(null);
                    }
                  }}
                >
                  <Field label="Name">
                    <input
                      autoFocus
                      value={name}
                      onChange={(e) => setName(e.target.value)}
                      placeholder="e.g. code-review"
                      maxLength={64}
                    />
                  </Field>
                  <Field label="Start from">
                    <select value={template} onChange={(e) => setTemplate(e.target.value)}>
                      <option value="blank">Blank graph</option>
                      {bootstrap?.templates.map((t) => (
                        <option key={t.id ?? t.name} value={t.id ?? t.name}>
                          {t.label ??
                            (t.name === 'single-worker' ? 'Single worker' : 'Software change')}
                        </option>
                      ))}
                    </select>
                  </Field>
                  <div className="modal-actions">
                    <button type="button" className="button" onClick={closeModal}>
                      Cancel
                    </button>
                    <button className="button primary" type="submit">
                      Create profile
                    </button>
                  </div>
                </form>
              )}
              {modal === 'save' && (
                <form
                  onSubmit={(e) => {
                    e.preventDefault();
                    void save(name, copy);
                  }}
                >
                  <Field label="Profile name">
                    <input
                      autoFocus
                      value={name}
                      disabled={busy}
                      onChange={(e) => setName(e.target.value)}
                      placeholder="e.g. code-review"
                    />
                    <small>
                      Letters, numbers, dashes, dots and underscores. Up to 64 characters.
                    </small>
                  </Field>
                  {modalError && (
                    <p className="modal-error" role="alert">
                      {modalError}
                    </p>
                  )}
                  <div className="modal-actions">
                    <button type="button" className="button" disabled={busy} onClick={closeModal}>
                      Cancel
                    </button>
                    <button className="button primary" type="submit" disabled={busy}>
                      {busy ? 'Saving…' : 'Save profile'}
                    </button>
                  </div>
                </form>
              )}
              {(modal === 'json' || modal === 'import') && (
                <>
                  {modal === 'import' && (
                    <label className="file-input">
                      <Upload size={16} /> Choose JSON file
                      <input
                        type="file"
                        accept=".json,application/json"
                        onChange={async (e) => {
                          const file = e.target.files?.[0];
                          if (!file) return;
                          if (file.size > 2 * 1024 * 1024) {
                            setModalError('Choose a profile smaller than 2 MiB.');
                            return;
                          }
                          setJson(await file.text());
                          setModalError('');
                        }}
                      />
                    </label>
                  )}
                  <textarea
                    autoFocus
                    spellCheck={false}
                    className="json-editor"
                    aria-label={modal === 'import' ? 'Profile JSON' : 'JSON source'}
                    value={json}
                    onChange={(e) => setJson(e.target.value)}
                    placeholder={
                      '{\n  "name": "my-profile",\n  "graph": { ... },\n  "runtime": { ... }\n}'
                    }
                  />
                  {modalError && (
                    <p className="modal-error" role="alert">
                      {modalError}
                    </p>
                  )}
                  <div className="modal-actions">
                    <button className="button" onClick={closeModal}>
                      Cancel
                    </button>
                    <button
                      className="button primary"
                      onClick={modal === 'import' ? importJson : applyJson}
                    >
                      {modal === 'import' ? 'Import profile' : 'Apply changes'}
                    </button>
                  </div>
                </>
              )}
            </Modal>
          )}
          {confirm && (
            <Modal title={confirm.title} close={() => setConfirm(null)}>
              <p>{confirm.body}</p>
              <div className="modal-actions">
                <button autoFocus className="button" onClick={() => setConfirm(null)}>
                  Keep editing
                </button>
                <button
                  className="button primary"
                  onClick={() => {
                    confirm.action();
                    setConfirm(null);
                  }}
                >
                  Discard{confirm.title.startsWith('Remove') ? ' and remove' : ''}
                </button>
              </div>
            </Modal>
          )}
        </div>
        {host && hosted.showingRun && (
          <div className="app embedded-app">{renderRun?.(hosted.runId)}</div>
        )}
        {!host && historyPage && <StandaloneRuns services={services} bootstrap={bootstrap} />}
        {defaultsPage && (
          <DefaultsPage
            back={() => {
              window.location.hash = '';
            }}
          />
        )}
      </AuthoringProvider>
    </NumericDraftProvider>
  );
}
function Modal({
  title,
  close,
  children,
  wide = false,
}: {
  title: string;
  close: () => void;
  children: React.ReactNode;
  wide?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    const opener = document.activeElement as HTMLElement | null;
    dialog?.showModal();
    (
      (dialog?.querySelector('input, textarea, select') as HTMLElement | null) ??
      (dialog?.querySelector('.modal-actions button') as HTMLElement | null)
    )?.focus();
    return () => {
      dialog?.close();
      queueMicrotask(() => {
        if (opener?.isConnected) opener.focus();
      });
    };
  }, []);
  return (
    <dialog
      ref={ref}
      className={`modal ${wide ? 'wide' : ''}`}
      aria-label={title}
      onCancel={(e) => {
        e.preventDefault();
        close();
      }}
    >
      <div className="modal-heading">
        <h2>{title}</h2>
        <button className="icon-button" aria-label="Close dialog" onClick={close}>
          <X size={18} />
        </button>
      </div>
      {children}
    </dialog>
  );
}
