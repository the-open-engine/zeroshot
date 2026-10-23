//! Local, resumable host-agent initialization. Completed runs are read-only evidence sources.
//! Specialists receive task files; only the host bridge can validate and publish mutations.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use openengine_cluster_protocol::RunId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::proposals::LessonContext;
use crate::active_memory::{ActiveMemoryError, ActiveMemoryStore, MemorySnapshot};

mod checkouts;
mod discoveries;
mod files;
mod filters;
mod history;
mod preparation;
mod progress;
mod publication;
mod publish;
pub(crate) use progress::init;
pub(crate) use filters::Ignore;
mod tasks;

type Result<T> = std::result::Result<T, InitializationError>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum InitializationError {
    #[error("{0}")]
    Invalid(String),
    #[error("initialization file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid initialization JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Memory(#[from] ActiveMemoryError),
}

fn invalid(message: impl Into<String>) -> InitializationError {
    InitializationError::Invalid(message.into())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    schema_version: u32,
    state_root: PathBuf,
    filters: Ignore,
    runs: Vec<HistoricalRun>,
    snapshot: MemorySnapshot,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoricalRun {
    run_id: RunId,
    title: String,
    graph_hash: String,
    graph_name: String,
    workspace: PathBuf,
    repository: Option<String>,
    remote_key: Option<String>,
    repository_aliases: BTreeSet<String>,
    repository_enabled: bool,
    graph_enabled: bool,
    tasks: Vec<tasks::TaskRef>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    runs: Vec<SelectedRun>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedRun {
    run_id: RunId,
    include: bool,
    reason: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    schema_version: u32,
    source_digest: String,
    workspaces: BTreeSet<PathBuf>,
    snapshot: MemorySnapshot,
    new_ids: BTreeSet<String>,
    tasks: Vec<ReviewTask>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewTask {
    run: HistoricalRun,
    task: tasks::TaskRef,
}

impl Session {
    fn store(&self) -> Result<ActiveMemoryStore> {
        Ok(ActiveMemoryStore::open(
            &self.state_root.join("active-memory/lessons.sqlite3"),
        )?)
    }

    fn lesson_context<'a>(
        &'a self,
        run: &'a HistoricalRun,
        snapshot: &'a MemorySnapshot,
    ) -> LessonContext<'a> {
        LessonContext {
            snapshot,
            repository: &run.repository,
            graph_hash: &run.graph_hash,
            workspace: &run.workspace,
        }
    }
    fn scoped_snapshot(&self, run: &HistoricalRun) -> MemorySnapshot {
        MemorySnapshot {
            generation: self.snapshot.generation,
            lessons: self
                .snapshot
                .lessons
                .iter()
                .filter(|lesson| run.accepts(&lesson.scope))
                .cloned()
                .collect(),
        }
    }
}

fn load(directory: &Path) -> Result<Session> {
    files::private_directory(directory)?;
    let session: Session = files::read(&directory.join("session.json"))?;
    if session.schema_version != 2 || !session.state_root.is_absolute() {
        return Err(invalid("unsupported initialization session"));
    }
    for run in &session.runs {
        validate_paths(&run.workspace, &session.state_root, directory)?;
    }
    Ok(session)
}

pub(super) async fn prepare(
    state_root: PathBuf,
    directory: &Path,
    filters: Ignore,
) -> Result<Value> {
    let directory = std::path::absolute(directory)?;
    let Some((mut session, mut excluded, _lock)) =
        prepare_session(state_root, &directory, filters).await?
    else {
        return files::read(&directory.join("inventory.json"));
    };
    session.snapshot = history::snapshot(&session, &session.runs)?;
    export_discoveries(&mut session, &directory, &mut excluded).await;
    let report = initial_files(&session, &directory, excluded)?;
    files::write(&directory.join("prepared.json"), &true)?;
    Ok(report)
}

async fn export_discoveries(session: &mut Session, directory: &Path, excluded: &mut Vec<Value>) {
    for mut run in std::mem::take(&mut session.runs) {
        match tasks::discovery(session, &run, directory).await {
            Ok(tasks) => {
                run.tasks = tasks;
                session.runs.push(run);
            }
            Err(error) => excluded.push(json!({"run_id":run.run_id,"reason":error.to_string()})),
        }
    }
}

async fn prepare_session(
    state_root: PathBuf,
    directory: &Path,
    filters: Ignore,
) -> Result<Option<(Session, Vec<Value>, preparation::SessionLock)>> {
    let Some(lock) = preparation::checkpoint(&state_root, &filters, directory)? else {
        return Ok(None);
    };
    let (runs, excluded) = history::inventory(&state_root, directory, &filters).await?;
    let session = Session {
        schema_version: 2,
        state_root,
        filters,
        runs,
        snapshot: MemorySnapshot::default(),
    };
    Ok(Some((session, excluded, lock)))
}

pub(super) fn validate_paths(workspace: &Path, state: &Path, directory: &Path) -> Result<()> {
    if !workspace.is_absolute() {
        return Err(invalid("recorded workspace must be absolute"));
    }
    crate::native_v2_local::validate_workspace_state(workspace, state)
        .map_err(|_| invalid("workspace overlaps local state"))?;
    crate::native_v2_local::validate_workspace_state(workspace, directory)
        .map_err(|_| invalid("session must be outside the checkout"))?;
    Ok(())
}

fn initial_files(session: &Session, directory: &Path, excluded: Vec<Value>) -> Result<Value> {
    files::write(&directory.join("session.json"), session)?;
    let selection = Selection {
        runs: session
            .runs
            .iter()
            .map(|run| SelectedRun {
                run_id: run.run_id.clone(),
                include: false,
                reason: "unclassified".to_owned(),
            })
            .collect(),
    };
    files::write(&directory.join("selection.json"), &selection)?;
    let report = json!({"session":directory,"runs":session.runs,
        "excluded":excluded,"tool_receipts_available":false,
        "filters":session.filters,
        "next":"Classify every candidate in selection.json; run both exported discovery specialists for every included run, then call memory init again."});
    files::write(&directory.join("inventory.json"), &report)?;
    Ok(report)
}

fn selected_runs(session: &Session, directory: &Path) -> Result<Vec<HistoricalRun>> {
    let selection: Selection = files::read(&directory.join("selection.json"))?;
    let mut pending = session
        .runs
        .iter()
        .map(|run| (run.run_id.as_str(), run))
        .collect::<BTreeMap<_, _>>();
    let mut selected = Vec::new();
    for entry in selection.runs {
        let run = pending
            .remove(entry.run_id.as_str())
            .ok_or_else(|| invalid("duplicate or foreign run selection"))?;
        if entry.reason.trim().is_empty() || entry.reason == "unclassified" {
            return Err(invalid("classify every run with an explicit reason"));
        }
        if entry.include {
            selected.push(run.clone());
        }
    }
    if !pending.is_empty() {
        return Err(invalid("run selection is incomplete"));
    }
    selected.sort_by(|left, right| right.run_id.as_str().cmp(left.run_id.as_str()));
    Ok(selected)
}

pub(super) async fn review(directory: &Path) -> Result<Value> {
    let session = load(directory)?;
    let runs = selected_runs(&session, directory)?;
    if runs.is_empty() {
        return Err(invalid("no qualified runs selected; nothing to initialize"));
    }
    let snapshot = history::snapshot(&session, &runs)?;
    let (snapshot, new_ids) = discoveries::collect(&session, &runs, snapshot, directory).await?;
    finish_review(&session, directory, &runs, (snapshot, new_ids)).await
}

async fn refresh_review(session: &Session, directory: &Path) -> Result<Value> {
    let previous: Review = files::read(&directory.join("review.json"))?;
    let runs = selected_runs(session, directory)?;
    let mut snapshot = history::snapshot(session, &runs)?;
    let store = session.store()?;
    let mut new_ids = BTreeSet::new();
    for lesson in previous.snapshot.lessons {
        if previous.new_ids.contains(&lesson.id) && store.record(&lesson.id)?.is_none() {
            new_ids.insert(lesson.id.clone());
            snapshot.lessons.push(lesson);
        }
    }
    finish_review(session, directory, &runs, (snapshot, new_ids)).await
}

async fn finish_review(
    session: &Session,
    directory: &Path,
    runs: &[HistoricalRun],
    staged: (MemorySnapshot, BTreeSet<String>),
) -> Result<Value> {
    let (snapshot, new_ids) = staged;
    let workspaces = runs.iter().map(|run| run.workspace.clone()).collect();
    let source_digest = checkouts::sources_digest(session, &workspaces).await?;
    let mut review = Review {
        schema_version: 2,
        source_digest,
        workspaces,
        snapshot,
        new_ids,
        tasks: Vec::new(),
    };
    tasks::final_reviews(session, &mut review, directory, runs).await?;
    files::write(&directory.join("review.json"), &review)?;
    Ok(
        json!({"staged_new_lessons":review.new_ids.len(),"lessons_to_reconcile":review.snapshot.lessons.len(),
        "maintenance_tasks":review.tasks,"source_digest":review.source_digest,
        "next":"Use one graph maintenance subagent and one repository maintenance subagent for their entire task sets, then call memory init again."}),
    )
}

pub(super) async fn commit(directory: &Path) -> Result<Value> {
    let session = load(directory)?;
    publish::commit(&session, directory).await
}

impl Review {
    async fn verify_source(&self, session: &Session) -> Result<()> {
        if self.schema_version != 2
            || checkouts::sources_digest(session, &self.workspaces).await? != self.source_digest
        {
            return Err(invalid(
                "checkout changed since review; call memory init again to refresh maintenance",
            ));
        }
        Ok(())
    }
}

impl HistoricalRun {
    fn accepts(&self, scope: &crate::active_memory::LessonScope) -> bool {
        use crate::active_memory::LessonScope;
        match scope {
            LessonScope::Repository { repository, .. } => {
                self.repository_enabled && self.repository.as_ref() == Some(repository)
            }
            LessonScope::Graph {
                graph_hash,
                repository,
                ..
            } => {
                self.graph_enabled
                    && graph_hash == &self.graph_hash
                    && repository
                        .as_ref()
                        .is_none_or(|repository| self.repository.as_ref() == Some(repository))
            }
        }
    }
}
