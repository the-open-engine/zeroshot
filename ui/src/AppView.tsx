import { useEffect, useRef } from 'react';
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
import { RuntimeEditor } from './RuntimeEditor';
import { AuthoringProvider } from './AuthoringProvider';
import { WorkflowCanvas } from './WorkflowCanvas';
import { parallelSiblingCandidates } from './workflow-authoring';
import { GuardEditor } from './GuardEditor';
import { Field, Inspector } from './Inspector';
import { NumericDraftProvider } from './numeric-drafts';
import {
  addNode,
  blankDocument,
  editableKinds,
  labels,
  replaceBody,
  allNodes,
  assertDocument,
  connectAfter,
  findNode,
  isGroup,
  newDocument,
  pathTo,
  removeNode,
  reorder,
  replaceNode,
} from './domain';
import { AppHeader } from './AppHeader';
import { StandaloneRuns } from './StandaloneRuns';
import { DefaultsPage } from './DefaultsPage';
import type { AppModel } from './App';
const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

export function AppView({ model }: { model: AppModel }) {
  const {
    numericDrafts,
    defaultsPage,
    historyPage,
    hosted,
    setSidebar,
    sidebar,
    setConfirm,
    confirm,
    services,
    bootstrap,
    host,
    renderRun,
  } = model;
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
                model.guard(() => {
                  window.location.hash = section === 'profiles' ? '' : section;
                });
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
            {!host && <ProfileSidebar model={model} />}
            <ProfileWorkspace model={model} />
          </div>
          <AppDialogs model={model} />
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

function AppDialogs({ model }: { model: AppModel }) {
  const {
    modal,
    workflowAction,
    copy,
    jsonTarget,
    closeModal,
    bootstrap,
    doc,
    modalError,
    apply,
    setModal,
    openJson,
    adopt,
    name,
    setName,
    template,
    setTemplate,
    busy,
    save,
  } = model;
  return (
    <>
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
          <WorkflowActionContent model={model} />
          <OutcomeContent model={model} />
          {modal === 'runtime' && doc && (
            <RuntimeEditor
              document={doc}
              schema={bootstrap?.runtimeSchema}
              edit={apply}
              openJson={() => openJson('runtime')}
            />
          )}
          <BodyContent model={model} />
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
                        t.name.replaceAll('-', ' ').replace(/^\w/, (c) => c.toUpperCase())}
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
                <small>Letters, numbers, dashes, dots and underscores. Up to 64 characters.</small>
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
          <JsonContent model={model} />
        </Modal>
      )}
    </>
  );
}

function WorkflowActionContent({ model }: { model: AppModel }) {
  const {
    modal,
    workflowAction,
    bodyKind,
    setBodyKind,
    bootstrap,
    doc,
    modalError,
    applyWorkflowAction,
    busy,
    closeModal,
  } = model;
  return (
    <>
      {modal === 'workflow-action' && workflowAction && (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            applyWorkflowAction();
          }}
        >
          {!['parallel', 'repeat', 'review'].includes(workflowAction.kind) && (
            <Field label="Activity type">
              <select value={bodyKind} onChange={(event) => setBodyKind(event.target.value)}>
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
          {workflowAction.kind === 'parallel' && doc && <ParallelActivities model={model} />}
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
    </>
  );
}

function OutcomeContent({ model }: { model: AppModel }) {
  const { modal, doc, outcome, apply, setModal, reveal } = model;
  return (
    <>
      {modal === 'outcome' &&
        doc &&
        outcome?.owner &&
        (() => {
          const choice = findNode(doc.graph.root, outcome.owner!);
          if (choice?.kind !== 'choice') return <p>This path no longer belongs to a decision.</p>;
          const branch =
            outcome.branchIndex !== undefined ? choice.branches[outcome.branchIndex] : undefined;
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
    </>
  );
}

function BodyContent({ model }: { model: AppModel }) {
  const {
    modal,
    doc,
    bodyParent,
    bodyKind,
    setBodyKind,
    apply,
    reveal,
    setModal,
    setModalError,
    modalError,
    closeModal,
  } = model;
  return (
    <>
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
              Replace the current body of <strong>{bodyParent}</strong>. Its nodes and runtime
              settings will be removed. References elsewhere may need updating.
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
    </>
  );
}

function JsonContent({ model }: { model: AppModel }) {
  const { modal, setJson, setModalError, json, modalError, closeModal, importJson, applyJson } =
    model;
  return (
    <>
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
            placeholder={'{\n  "name": "my-profile",\n  "graph": { ... },\n  "runtime": { ... }\n}'}
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
    </>
  );
}

function ProfileWorkspace({ model }: { model: AppModel }) {
  const {
    loading,
    doc,
    dirty,
    base,
    pendingEdits,
    past,
    busy,
    undo,
    future,
    redo,
    exportDocument,
    host,
    saveDialog,
    inspector,
    selected,
    positions,
    setPositions,
    bootstrap,
    hosted,
    reveal,
    workflowFocus,
    beginWorkflowAction,
    setOutcome,
    setModal,
    attempt,
    apply,
    openJson,
    setInspector,
    documentGeneration,
    inspectorReset,
    node,
    parent,
    editNode,
    setConfirm,
    setSelected,
    bodyDialog,
    wrap,
    setBodyParent,
    setBodyKind,
    setModalError,
    validation,
    setIssues,
    issues,
    notice,
    error,
    setError,
  } = model;
  return (
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
                    if (sourceParent?.kind !== 'seq' || sourceParent.name !== targetParent?.name)
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
                allNodes(doc.graph.root).filter((node) => ['step', 'verifier'].includes(node.kind))
                  .length
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
          ) : host ? (
            <span>Select or create a profile to start.</span>
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
          <button className="icon-button" aria-label="Dismiss error" onClick={() => setError('')}>
            <X size={16} />
          </button>
        </div>
      )}
    </main>
  );
}

function ProfileSidebar({ model }: { model: AppModel }) {
  const {
    sidebar,
    busy,
    loading,
    guard,
    setName,
    setModalError,
    setModal,
    profiles,
    doc,
    base,
    load,
    setJson,
  } = model;
  return (
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
        {!profiles.length && !loading && <div className="no-profiles">No saved profiles</div>}
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

function ParallelActivities({ model }: { model: AppModel }) {
  const { doc, workflowAction, parallelActivities, setParallelActivities, setModalError } = model;
  if (!doc || !workflowAction) return null;
  const candidates = parallelSiblingCandidates(doc, workflowAction.owner);
  if (candidates.length < 2) return null;
  const owner = candidates.findIndex((node) => node.name === workflowAction.owner);
  function select(index: number, checked: boolean) {
    setParallelActivities((current) =>
      candidates
        .filter(
          (candidate, at) =>
            at !== owner &&
            (checked
              ? current.includes(candidate.name) ||
                (at >= Math.min(owner, index) && at <= Math.max(owner, index))
              : current.includes(candidate.name) && (index < owner ? at > index : at < index))
        )
        .map((candidate) => candidate.name)
    );
    setModalError('');
  }
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
            onChange={(event) => select(index, event.target.checked)}
          />
          {node.name}
        </label>
      ))}
    </fieldset>
  );
}
