use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use openengine_cluster_protocol::{RunId, TerminalResult, is_canonical_uuid_v7};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{HistoricalRun, Ignore, Result, Session, files, invalid};
use super::super::{MainRunFacts, evidence};
use crate::active_memory::{MemorySnapshot, ScopeQuery};
use crate::active_memory::runtime::{authored_graph_hash, repository_key};
use crate::v2_run_ledger::{RunLedger, RunPhase, StoredRun, sqlite::SqliteRunLedger};

pub(super) struct LoadedRun {
    run_id: RunId,
    stored: StoredRun,
    terminal: TerminalResult,
    ledger: SqliteRunLedger,
}

impl LoadedRun {
    pub(super) fn facts(&self) -> MainRunFacts<'_> {
        MainRunFacts {
            run_id: &self.run_id,
            admitted: &self.stored.admitted,
            snapshot: &self.stored.snapshot,
            primary_terminal: &self.terminal,
            ledger: &self.ledger,
        }
    }

    pub(super) fn proposal_context<'a>(
        &'a self,
        context: super::super::proposals::LessonContext<'a>,
        evidence: &'a evidence::FrozenEvidence,
        supplied: Option<crate::active_memory::SuppliedMemoryManifest>,
    ) -> super::super::proposals::ProposalContext<'a> {
        super::super::proposals::ProposalContext::new(context, self.facts(), evidence, supplied)
    }

    pub(super) fn export_graph(&self, directory: &Path) -> Result<PathBuf> {
        let path = directory.join(format!("{}-graph.json", self.run_id.as_str()));
        files::write(&path, &self.stored.admitted.graph)?;
        Ok(path)
    }

    pub(super) async fn evidence(&self) -> Result<evidence::FrozenEvidence> {
        let mut evidence = evidence::collect(self.facts(), Vec::new())
            .await
            .map_err(|_| invalid("historical main-run evidence unavailable"))?;
        evidence.document["initialization"] = json!({"historical":true,"native_tool_receipts_available":false,
            "limitation":"Tool exploration receipts were not persisted; only durable proven failure/review episodes can qualify. Do not infer tool activity from model latency."});
        Ok(evidence)
    }
}

pub(super) struct LoadedDiscovery {
    loaded: LoadedRun,
    evidence: evidence::FrozenEvidence,
    snapshot: MemorySnapshot,
}

pub(super) async fn discovery(session: &Session, run: &HistoricalRun) -> Result<LoadedDiscovery> {
    let loaded = open(&session.state_root, &run.run_id).await?;
    let evidence = loaded.evidence().await?;
    let snapshot = session.scoped_snapshot(run);
    Ok(LoadedDiscovery {
        loaded,
        evidence,
        snapshot,
    })
}

impl LoadedDiscovery {
    pub(super) fn context<'a>(
        &'a self,
        session: &'a Session,
        run: &'a HistoricalRun,
    ) -> super::super::proposals::ProposalContext<'a> {
        self.loaded.proposal_context(
            session.lesson_context(run, &self.snapshot),
            &self.evidence,
            None,
        )
    }
    pub(super) fn export_graph(&self, directory: &Path) -> Result<PathBuf> {
        self.loaded.export_graph(directory)
    }
}

pub(super) async fn open(root: &Path, run_id: &RunId) -> Result<LoadedRun> {
    if !is_canonical_uuid_v7(run_id) {
        return Err(invalid("invalid local history run ID"));
    }
    let path = ledger_path(root, run_id)?;
    let ledger = SqliteRunLedger::open_read_only(&path)
        .map_err(|_| invalid("cannot open historical ledger read-only"))?;
    let stored = ledger
        .get(run_id)
        .await
        .map_err(|_| invalid("cannot read historical run"))?
        .ok_or_else(|| invalid("historical run missing"))?;
    let terminal = finished_primary(&stored)?;
    Ok(LoadedRun {
        run_id: run_id.clone(),
        stored,
        terminal,
        ledger,
    })
}

fn ledger_path(root: &Path, run_id: &RunId) -> Result<PathBuf> {
    let directory = root.join("runs").join(run_id.as_str());
    files::reject_links(&directory)?;
    let metadata = fs::symlink_metadata(&directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid("unsafe history directory"));
    }
    let path = directory.join("runs.sqlite3");
    if !fs::symlink_metadata(&path)?.is_file() {
        return Err(invalid("unsafe historical ledger"));
    }
    Ok(path)
}

fn finished_primary(stored: &StoredRun) -> Result<TerminalResult> {
    let snapshot = &stored.snapshot;
    if snapshot.phase != RunPhase::Finished
        || snapshot.terminal.is_none()
        || snapshot.active_executions().next().is_some()
    {
        return Err(invalid("run is unfinished or cleanup is unconfirmed"));
    }
    snapshot
        .primary_graph
        .as_ref()
        .map(|primary| primary.result.clone())
        .or_else(|| snapshot.terminal.clone())
        .ok_or_else(|| invalid("primary outcome unavailable"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceProjection {
    workspace: PathBuf,
}

pub(super) async fn inventory(
    root: &Path,
    directory: &Path,
    filters: &Ignore,
) -> Result<(Vec<HistoricalRun>, Vec<Value>)> {
    let entries = match fs::read_dir(root.join("runs")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Vec::new()));
        }
        Err(error) => return Err(error.into()),
    };
    let mut ids = entries
        .map(|entry| entry.map(entry_id))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    ids.sort_by(|left, right| right.as_str().cmp(left.as_str()));
    let mut checkouts = super::checkouts::Cache::new();
    let mut runs = Vec::new();
    let mut excluded = Vec::new();
    for id in ids {
        match qualify(
            &InventoryContext {
                root,
                directory,
                filters,
            },
            &id,
            &mut checkouts,
        )
        .await
        {
            Ok(run) => runs.push(run),
            Err(error) => excluded.push(json!({"run_id":id,"reason":error.to_string()})),
        }
    }
    Ok((runs, excluded))
}

fn entry_id(entry: fs::DirEntry) -> Option<RunId> {
    let name = entry.file_name().to_str()?.to_owned();
    let id = RunId::new(name);
    is_canonical_uuid_v7(&id).then_some(id)
}

struct InventoryContext<'a> {
    root: &'a Path,
    directory: &'a Path,
    filters: &'a Ignore,
}

async fn qualify(
    context: &InventoryContext<'_>,
    run_id: &RunId,
    cache: &mut super::checkouts::Cache,
) -> Result<HistoricalRun> {
    let loaded = open(context.root, run_id).await?;
    let admitted = &loaded.stored.admitted;
    let graph_hash = authored_graph_hash(admitted)?;
    let graph_name = admitted.graph.root.name().as_str().to_owned();
    if context.filters.excludes_graph(&graph_name, &graph_hash) {
        return Err(invalid("graph is ignored"));
    }
    ensure_collectible(&loaded)?;
    let checkout = resolve_checkout(context, &loaded, cache).await?;
    let config = admitted.runtime.active_memory();
    Ok(HistoricalRun {
        run_id: run_id.clone(),
        title: admitted.title.as_str().to_owned(),
        graph_hash,
        graph_name,
        workspace: checkout.workspace,
        repository: Some(checkout.repository),
        remote_key: checkout.remote_key,
        repository_aliases: checkout.aliases,
        repository_enabled: config.repository,
        graph_enabled: config.graph,
        tasks: Vec::new(),
    })
}

async fn resolve_checkout(
    context: &InventoryContext<'_>,
    loaded: &LoadedRun,
    cache: &mut super::checkouts::Cache,
) -> Result<super::checkouts::Checkout> {
    let projection: WorkspaceProjection = files::read(
        &context
            .root
            .join("runs")
            .join(loaded.run_id.as_str())
            .join("workspace-recovery.json"),
    )?;
    let admitted = &loaded.stored.admitted;
    let mut aliases =
        super::filters::repository_aliases(&projection.workspace, admitted.source.as_ref());
    let historical_key = repository_key(admitted, &projection.workspace);
    aliases.insert(historical_key.clone());
    if context.filters.excludes_repository(&aliases) {
        return Err(invalid("repository is ignored"));
    }
    let mut checkout = super::checkouts::resolve(
        &projection.workspace,
        (context.root, context.directory),
        cache,
    )
    .await?;
    if admitted.source.is_some() && !checkout.aliases.contains(&historical_key) {
        return Err(invalid(
            "recorded checkout now has a different repository identity",
        ));
    }
    checkout.aliases.extend(aliases);
    if context.filters.excludes_repository(&checkout.aliases) {
        return Err(invalid("repository is ignored"));
    }
    Ok(checkout)
}

fn ensure_collectible(loaded: &LoadedRun) -> Result<()> {
    let admitted = &loaded.stored.admitted;
    let config = admitted.runtime.active_memory();
    if !config.enabled() {
        return Err(invalid("repository and graph memory were opted out"));
    }
    if !loaded.stored.snapshot.executions.values().any(|node| {
        matches!(
            admitted.runtime.nodes().get(&node.reference.node),
            Some(crate::native_v2_contract::NodeRuntimeBinding::Agent { .. })
        )
    }) {
        return Err(invalid("no main agent execution evidence"));
    }
    Ok(())
}

pub(super) fn snapshot(session: &Session, runs: &[HistoricalRun]) -> Result<MemorySnapshot> {
    let store = session.store()?;
    let mut lessons = BTreeMap::new();
    let mut generation = 0;
    for run in runs {
        let snapshot = store.snapshot(&ScopeQuery {
            repository: run.repository.clone(),
            graph_hash: Some(run.graph_hash.clone()),
            node_id: None,
            repository_lessons: run.repository_enabled,
            graph_lessons: run.graph_enabled,
        })?;
        generation = snapshot.generation;
        for lesson in snapshot.lessons {
            lessons.insert(lesson.id.clone(), lesson);
        }
    }
    Ok(MemorySnapshot {
        generation,
        lessons: lessons.into_values().collect(),
    })
}
