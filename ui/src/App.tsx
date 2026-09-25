import { useCallback, useEffect, useRef, useState } from 'react';
import { AppView } from './AppView';
import { applyWorker } from './workers';
import type { WorkflowAction } from './WorkflowCanvas';
import {
  addNext,
  addOutcome,
  appendActivity,
  automaticProtectionTarget,
  wrapActivity,
  wrapParallelActivities,
} from './workflow-authoring';
import type { WorkflowEdge } from './workflow-projection';
import { type Bootstrap, type Summary } from './api';
import type { WorkspaceServices } from './workspace-services';
import { createHostedProfile, useHostWorkspace } from './use-host-workspace';
import { workspaceStorageKeys } from './workspace-storage';
import { acknowledgeProfileSave, snapshotProfileSave } from './profile-save';
import { createNumericDrafts } from './numeric-drafts';
import {
  wrapBody,
  allNodes,
  assertDocument,
  children,
  clone,
  findNode,
  isGroup,
  newDocument,
  pathTo,
  replaceNode,
  type Document,
  type GraphNode,
  type Positions,
} from './domain';
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
type AppProps = {
  services: WorkspaceServices;
  bootstrap: Bootstrap;
  host?: Parameters<typeof useHostWorkspace>[0];
  renderRun?: (runId?: string) => React.ReactNode;
};
export function App(props: AppProps) {
  const model = useAppController(props);
  return <AppView model={model} />;
}
function useAppState({ services, bootstrap, host }: AppProps) {
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
  return {
    storage,
    numericDrafts,
    historyPage,
    defaultsPage,
    profiles,
    setProfiles,
    doc,
    setDoc,
    base,
    setBase,
    revision,
    setRevision,
    selected,
    setSelected,
    positions,
    setPositions,
    inspector,
    setInspector,
    sidebar,
    setSidebar,
    inspectorReset,
    setInspectorReset,
    workflowFocus,
    setWorkflowFocus,
    workflowAction,
    setWorkflowAction,
    parallelActivities,
    setParallelActivities,
    outcome,
    setOutcome,
    past,
    setPast,
    future,
    setFuture,
    error,
    setError,
    notice,
    setNotice,
    busy,
    setBusy,
    loading,
    setLoading,
    validation,
    setValidation,
    issues,
    setIssues,
    modal,
    setModal,
    name,
    setName,
    template,
    setTemplate,
    json,
    setJson,
    jsonInitial,
    setJsonInitial,
    jsonTarget,
    setJsonTarget,
    modalError,
    setModalError,
    copy,
    setCopy,
    bodyParent,
    setBodyParent,
    bodyKind,
    setBodyKind,
    confirm,
    setConfirm,
    current,
    currentRevision,
    documentGeneration,
    savePending,
    saveAction,
    editTime,
    dirty,
    pendingEdits,
    jsonDirty,
    refresh,
    positionKey,
    apply,
    adopt,
  };
}

function useProfileEffects(
  { services, bootstrap, host }: AppProps,
  state: ReturnType<typeof useAppState>,
  hosted: ReturnType<typeof useHostWorkspace>
) {
  const {
    storage,
    setLoading,
    setProfiles,
    setNotice,
    setError,
    doc,
    base,
    revision,
    positions,
    dirty,
    positionKey,
    setValidation,
    jsonDirty,
    numericDrafts,
    selected,
    setSelected,
    adopt,
  } = state;
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
}

function createWorkflowEditor(
  { services, bootstrap }: AppProps,
  state: ReturnType<typeof useAppState>
) {
  const {
    doc,
    loading,
    setJson,
    setJsonInitial,
    setJsonTarget,
    setModalError,
    setModal,
    busy,
    jsonDirty,
    setConfirm,
    jsonTarget,
    json,
    numericDrafts,
    selected,
    setSelected,
    apply,
    setInspectorReset,
    adopt,
    setNotice,
    setError,
    setInspector,
    setWorkflowFocus,
    setWorkflowAction,
    setParallelActivities,
    setBodyKind,
    workflowAction,
    bodyKind,
    parallelActivities,
    base,
    setBusy,
    current,
    setBodyParent,
  } = state;
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
  return {
    openJson,
    closeModal,
    applyJson,
    importJson,
    editNode,
    attempt,
    select,
    reveal,
    beginWorkflowAction,
    applyWorkflowAction,
    node,
    parent,
    bodyDialog,
    wrap,
    exportDocument,
  };
}

function useProfileActions(
  { services, bootstrap, host }: AppProps,
  state: ReturnType<typeof useAppState>
) {
  const {
    storage,
    numericDrafts,
    historyPage,
    defaultsPage,
    doc,
    setDoc,
    base,
    setBase,
    revision,
    setRevision,
    past,
    setPast,
    future,
    setFuture,
    busy,
    setBusy,
    loading,
    setLoading,
    current,
    currentRevision,
    documentGeneration,
    savePending,
    saveAction,
    editTime,
    dirty,
    pendingEdits,
    jsonDirty,
    validation,
    setModal,
    modal,
    setNotice,
    setError,
    setCopy,
    setName,
    setModalError,
    setConfirm,
    confirm,
    setIssues,
    refresh,
    adopt,
  } = state;
  saveAction.current = () => {
    void saveDialog();
  };
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
  useProfileEffects({ services, bootstrap, host }, state, hosted);
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
  return { hosted, undo, redo, saveDialog, guard, load, save };
}

function useAppController({ services, bootstrap, host, renderRun }: AppProps) {
  const state = useAppState({ services, bootstrap, host });
  const profile = useProfileActions({ services, bootstrap, host }, state);
  const workflow = createWorkflowEditor({ services, bootstrap }, state);
  return { services, bootstrap, host, renderRun, ...state, ...profile, ...workflow };
}

export type AppModel = ReturnType<typeof useAppController>;
