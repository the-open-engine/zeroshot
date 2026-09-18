//! Read-only history services. The HTTP adapter only selects a retained run and a replay cursor;
//! immutable definitions and events come from the same native ledger used by the CLI.
use std::convert::Infallible;
use std::path::{Path as FilePath, PathBuf};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::{stream, Stream, StreamExt};
use openengine_cluster_protocol::{is_canonical_uuid_v7, Cursor, GraphSpec, RunId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{ApiError, UiState};
use crate::native_v2_cli::{default_local_state_root, NativeV2CliError};
use crate::v2_run_ledger::sqlite::SqliteRunLedger;
use crate::v2_run_ledger::{
    cursor_sequence, initial_cursor, RunLedger, RunLedgerError, RunSnapshot, StoredRun,
};

mod control;
use control::{ControlCache, ControlRecord};
mod status;
use status::{RuntimeFailure, RuntimeStatusReader};

const LIST_PAGE_SIZE: usize = 50;
const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryPage {
    events: Vec<Value>,
    next_cursor: Cursor,
    head_cursor: Cursor,
    complete: bool,
    finished: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_failure: Option<RuntimeFailure>,
    #[serde(skip)]
    runtime_available: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    control: Vec<ControlRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    control_error: Option<String>,
}

struct LiveHistory {
    ledger: SqliteRunLedger,
    id: RunId,
    after: Cursor,
    first: Option<HistoryPage>,
    at_head: bool,
    done: bool,
    unavailable: bool,
    source: NativeRunHistory,
}

impl LiveHistory {
    async fn next(&mut self) -> Option<Result<HistoryPage, ApiError>> {
        if self.done {
            return None;
        }
        if self.unavailable {
            self.done = true;
            return Some(Err(runtime_unavailable()));
        }
        loop {
            let page = if let Some(page) = self.first.take() {
                page
            } else {
                if self.at_head {
                    tokio::time::sleep(LIVE_POLL_INTERVAL).await;
                }
                match read_page(&self.ledger, &self.id, self.after.clone(), &self.source).await {
                    Ok(page)
                        if page.events.is_empty() && !page.finished && page.runtime_available =>
                    {
                        continue;
                    }
                    Ok(page) => page,
                    Err(error) => {
                        self.done = true;
                        return Some(Err(error));
                    }
                }
            };
            self.after = page.next_cursor.clone();
            self.at_head = page.complete;
            self.done = page.complete && page.finished;
            self.unavailable = page.complete && !page.finished && !page.runtime_available;
            return Some(Ok(page));
        }
    }
}

#[derive(Clone)]
pub(super) struct NativeRunHistory {
    root: PathBuf,
    layout: LedgerLayout,
    control: ControlCache,
    status: Option<RuntimeStatusReader>,
}

#[derive(Clone, Copy)]
enum LedgerLayout {
    Local,
    Target,
}

impl NativeRunHistory {
    pub(super) fn production() -> Result<Self, NativeV2CliError> {
        let root = default_local_state_root()?;
        Ok(Self {
            status: Some(RuntimeStatusReader::Local(root.clone())),
            ..Self::new(root)
        })
    }

    pub(super) fn new(root: PathBuf) -> Self {
        Self {
            status: None,
            root,
            layout: LedgerLayout::Local,
            control: ControlCache::default(),
        }
    }

    pub(super) fn target(
        root: PathBuf,
        observations: crate::native_v2_observability::NativeV2Observability,
    ) -> Self {
        Self {
            root,
            layout: LedgerLayout::Target,
            control: ControlCache::default(),
            status: Some(RuntimeStatusReader::Target(observations)),
        }
    }

    async fn open(&self, id: &RunId) -> Result<SqliteRunLedger, ApiError> {
        let directory = match self.layout {
            LedgerLayout::Local => self.root.join("runs").join(id.as_str()),
            LedgerLayout::Target => self.root.clone(),
        };
        open_ledger(directory).await
    }

    async fn ids(&self, after: Option<&RunId>) -> Result<Vec<RunId>, ApiError> {
        if matches!(self.layout, LedgerLayout::Target) {
            let ledger = match open_ledger(self.root.clone()).await {
                Ok(ledger) => ledger,
                Err(error) if error.status == StatusCode::NOT_FOUND => return Ok(Vec::new()),
                Err(error) => return Err(error),
            };
            return ledger
                .list_ids_page(after, (LIST_PAGE_SIZE + 1) as u16)
                .await
                .map_err(ledger_error);
        }
        let directory = self.root.join("runs");
        let after = after.cloned();
        tokio::task::spawn_blocking(move || {
            match std::fs::symlink_metadata(&directory) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                _ => require_directory(&directory)?,
            }
            let mut ids = Vec::new();
            for entry in std::fs::read_dir(directory).map_err(file_error)? {
                let entry = entry.map_err(file_error)?;
                let metadata = entry.file_type().map_err(file_error)?;
                if !metadata.is_dir() || metadata.is_symlink() {
                    continue;
                }
                if let Some(name) = entry.file_name().to_str() {
                    let id = RunId::new(name);
                    if is_canonical_uuid_v7(&id)
                        && after
                            .as_ref()
                            .is_none_or(|after| id.as_str() < after.as_str())
                    {
                        ids.push(id);
                    }
                }
            }
            ids.sort_by(|a, b| b.as_str().cmp(a.as_str()));
            ids.truncate(LIST_PAGE_SIZE + 1);
            Ok(ids)
        })
        .await
        .map_err(|_| unavailable())?
    }

    async fn stored(&self, id: &RunId) -> Result<StoredRun, ApiError> {
        self.open(id)
            .await?
            .get(id)
            .await
            .map_err(ledger_error)?
            .ok_or_else(not_found)
    }

    async fn runtime_observation(
        &self,
        snapshot: &RunSnapshot,
    ) -> Result<Option<RuntimeFailure>, ()> {
        match &self.status {
            Some(reader) => reader.failure(snapshot).await,
            None => Ok(None),
        }
    }

    async fn list(&self, after: Option<RunId>) -> Result<Value, ApiError> {
        let ids = self.ids(after.as_ref()).await?;
        let mut remaining = ids.into_iter();
        let page = remaining.by_ref().take(LIST_PAGE_SIZE).collect::<Vec<_>>();
        let next = remaining
            .next()
            .is_some()
            .then(|| page.last().cloned())
            .flatten();
        let runs = stream::iter(page)
            .map(|id| async move {
                match self.stored(&id).await {
                    Ok(stored) => {
                        let mut value = summary(&stored.snapshot);
                        if let Ok(Some(failure)) = self.runtime_observation(&stored.snapshot).await
                        {
                            failure.apply(&mut value);
                        }
                        value
                    }
                    Err(_) => json!({
                        "runId":id, "title":id, "phase":"unavailable", "cursor":null,
                        "terminal":null, "source":null, "createdAt":created_at(&id),
                        "historyAvailable":false, "issue":"history_unavailable"
                    }),
                }
            })
            .buffered(8)
            .collect::<Vec<_>>()
            .await;
        Ok(json!({"runs":runs,"nextCursor":next}))
    }

    async fn detail(&self, id: &RunId) -> Result<Value, ApiError> {
        let stored = self.stored(id).await?;
        let runtime_failure = self
            .runtime_observation(&stored.snapshot)
            .await
            .ok()
            .flatten();
        let graph = GraphSpec {
            profile: stored.admitted.graph.profile,
            initial_input: stored.admitted.graph.initial_input,
            policy: stored.admitted.graph.policy,
            root: stored.admitted.graph.root,
        };
        let mut snapshot = serde_json::to_value(&stored.snapshot).map_err(|_| unavailable())?;
        if let Some(executions) = snapshot["executions"].as_object_mut() {
            for node in executions.values_mut() {
                stringify_reference(&mut node["reference"]);
            }
        }
        let mut detail = json!({
            "version":1, "projectionVersion":1,
            "runId":id, "title":stored.admitted.title, "createdAt":created_at(id),
            "phase":stored.snapshot.phase,"cursor":stored.snapshot.cursor,
            "terminal":stored.snapshot.terminal,"historyAvailable":true,
            "graph":graph, "runtime":stored.admitted.runtime,
            "initialInput":stored.admitted.initial_input,"source":stored.admitted.source,
            "snapshot":snapshot,
            "history":{
                "initialCursor":initial_cursor(),"cursor":stored.snapshot.cursor,
                "complete":stored.snapshot.terminal.is_some(),
                "limitations":["control_flow_projected_by_reducer","retained_provider_output_only"]
            }
        });
        if let Some(failure) = runtime_failure {
            failure.apply(&mut detail);
        }
        Ok(detail)
    }

    async fn page(&self, id: &RunId, after: Option<Cursor>) -> Result<Value, ApiError> {
        let ledger = self.open(id).await?;
        read_page(&ledger, id, after.unwrap_or_else(initial_cursor), self)
            .await
            .and_then(|page| serde_json::to_value(page).map_err(|_| unavailable()))
    }

    async fn subscribe(
        &self,
        id: RunId,
        after: Option<Cursor>,
    ) -> Result<impl Stream<Item = Result<HistoryPage, ApiError>> + Send + 'static + use<>, ApiError>
    {
        let ledger = self.open(&id).await?;
        let after = after.unwrap_or_else(initial_cursor);
        let first = read_page(&ledger, &id, after.clone(), self).await?;
        let state = LiveHistory {
            ledger,
            id,
            after,
            first: Some(first),
            at_head: false,
            done: false,
            unavailable: false,
            source: self.clone(),
        };
        // The response owns the reader and timer. Disconnecting drops both; there is no
        // producer task or event queue that can outlive the HTTP subscription.
        Ok(stream::unfold(state, |mut state| async move {
            state.next().await.map(|page| (page, state))
        }))
    }
}

async fn read_page(
    ledger: &SqliteRunLedger,
    id: &RunId,
    after: Cursor,
    source: &NativeRunHistory,
) -> Result<HistoryPage, ApiError> {
    let start = cursor_sequence(&after).map_err(ledger_error)?;
    let observation = ledger
        .snapshot_and_tail(id, Some(&after))
        .await
        .map_err(ledger_error)?;
    let head = cursor_sequence(&observation.snapshot.cursor).map_err(ledger_error)?;
    let mut next = after;
    let mut sequence = start;
    let mut events = Vec::with_capacity(observation.events.len());
    for stored in observation.events {
        let event_sequence = cursor_sequence(&stored.cursor).map_err(ledger_error)?;
        // Another controller may append between the ledger's snapshot and event reads.
        // Keep this page pinned to the head that was actually observed.
        if event_sequence > head {
            break;
        }
        if event_sequence != sequence + 1 {
            return Err(history_gap());
        }
        sequence = event_sequence;
        next = stored.cursor.clone();
        let mut value = serde_json::to_value(stored).map_err(|_| unavailable())?;
        project_event(&mut value["event"]);
        events.push(value);
    }
    if events.is_empty() && sequence < head {
        return Err(history_gap());
    }
    let controls = source.control.project(ledger, id, start..sequence).await;
    let runtime = source.runtime_observation(&observation.snapshot).await;
    let runtime_available = runtime.is_ok();
    let runtime_failure = runtime.ok().flatten();
    let finished = observation.snapshot.terminal.is_some() || runtime_failure.is_some();
    Ok(HistoryPage {
        events,
        next_cursor: next,
        head_cursor: observation.snapshot.cursor,
        complete: sequence == head,
        finished,
        runtime_failure,
        runtime_available,
        control: controls.records,
        control_error: controls.error,
    })
}

fn summary(snapshot: &RunSnapshot) -> Value {
    json!({
        "runId":snapshot.run_id,"title":snapshot.title,"phase":snapshot.phase,
        "cursor":snapshot.cursor,"terminal":snapshot.terminal,"source":snapshot.source,
        "createdAt":created_at(&snapshot.run_id),"historyAvailable":true
    })
}

fn created_at(id: &RunId) -> Option<u64> {
    let (seconds, nanos) = uuid::Uuid::parse_str(id.as_str())
        .ok()?
        .get_timestamp()?
        .to_unix();
    seconds
        .checked_mul(1000)?
        .checked_add(u64::from(nanos / 1_000_000))
}

// Execution identifiers are opaque strings in the browser. Native u64 values must never pass
// through a JavaScript number, even though current short runs usually have small identities.
fn stringify_reference(reference: &mut Value) {
    for key in ["execution", "nodeInstance"] {
        if let Some(id) = reference[key].as_u64() {
            reference[key] = Value::String(id.to_string());
        }
    }
}

fn project_event(event: &mut Value) {
    match event["kind"].as_str() {
        Some("node_started" | "execution_voided") => stringify_reference(&mut event["reference"]),
        Some("node_completed") => stringify_reference(&mut event["completion"]["reference"]),
        Some("safe_log" | "token_usage_observed") => {
            if let Some(id) = event["execution"].as_u64() {
                event["execution"] = Value::String(id.to_string());
            }
        }
        _ => {}
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageQuery {
    after: Option<String>,
}

pub(super) async fn list(
    State(state): State<UiState>,
    query: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let query = query.map_err(|_| invalid_cursor())?.0;
    let after = query.after.map(parse_id).transpose()?;
    state.runs.list(after).await.map(Json)
}

pub(super) async fn show(
    State(state): State<UiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state.runs.detail(&parse_id(id)?).await.map(Json)
}

pub(super) async fn history(
    State(state): State<UiState>,
    Path(id): Path<String>,
    query: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let query = query.map_err(|_| invalid_cursor())?.0;
    let after = query.after.map(Cursor::new);
    state.runs.page(&parse_id(id)?, after).await.map(Json)
}

pub(super) async fn events(
    State(state): State<UiState>,
    Path(id): Path<String>,
    query: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let query = query.map_err(|_| invalid_cursor())?.0;
    let after = resume_cursor(query, &headers)?;
    let stop = state.shutdown.clone();
    let stream = state
        .runs
        .subscribe(parse_id(id)?, after)
        .await?
        .take_until(async move { stop.cancelled().await });
    Ok(Sse::new(stream.map(|page| {
        Ok(match page {
            Ok(page) => Event::default()
                .event("history")
                .id(page.next_cursor.as_str())
                .data(json!(page).to_string()),
            Err(error) => Event::default()
                .event("history_error")
                .data(json!({"code":error.code,"message":error.message}).to_string()),
        })
    }))
    .keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

fn resume_cursor(query: PageQuery, headers: &HeaderMap) -> Result<Option<Cursor>, ApiError> {
    let mut values = headers.get_all("last-event-id").iter();
    let after = match values.next() {
        Some(value) => {
            if values.next().is_some() {
                return Err(invalid_cursor());
            }
            Some(value.to_str().map_err(|_| invalid_cursor())?.to_owned())
        }
        None => query.after,
    };
    after
        .map(|value| {
            let cursor = Cursor::new(value);
            cursor_sequence(&cursor).map_err(ledger_error)?;
            Ok(cursor)
        })
        .transpose()
}

fn parse_id(value: String) -> Result<RunId, ApiError> {
    let id = RunId::new(value);
    if is_canonical_uuid_v7(&id) {
        Ok(id)
    } else {
        Err(not_found())
    }
}

fn require_directory(path: &FilePath) -> Result<(), ApiError> {
    let metadata = std::fs::symlink_metadata(path).map_err(file_error)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(unavailable())
    }
}

async fn open_ledger(directory: PathBuf) -> Result<SqliteRunLedger, ApiError> {
    tokio::task::spawn_blocking(move || {
        require_directory(&directory)?;
        if let Some(parent) = directory.parent() {
            require_directory(parent)?;
        }
        let path = directory.join("runs.sqlite3");
        let metadata = std::fs::symlink_metadata(&path).map_err(file_error)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(unavailable());
        }
        SqliteRunLedger::open_read_only(path).map_err(ledger_error)
    })
    .await
    .map_err(|_| unavailable())?
}

fn file_error(error: std::io::Error) -> ApiError {
    if error.kind() == std::io::ErrorKind::NotFound {
        not_found()
    } else {
        unavailable()
    }
}

fn ledger_error(error: RunLedgerError) -> ApiError {
    match error {
        RunLedgerError::RunNotFound => not_found(),
        RunLedgerError::InvalidCursor | RunLedgerError::CursorAhead => invalid_cursor(),
        _ => unavailable(),
    }
}

fn not_found() -> ApiError {
    ApiError {
        status: StatusCode::NOT_FOUND,
        code: "run_not_found",
        message: "This run has no retained history.".into(),
    }
}
fn unavailable() -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "history_unavailable",
        message: "This run's history is unavailable or uses an unsupported format.".into(),
    }
}
fn runtime_unavailable() -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "runtime_unavailable",
        message:
            "The run controller is unavailable. Retained history is shown; retry to reconnect."
                .into(),
    }
}
fn history_gap() -> ApiError {
    ApiError {
        status: StatusCode::CONFLICT,
        code: "history_gap",
        message: "The retained history has a gap; replay cannot continue.".into(),
    }
}
fn invalid_cursor() -> ApiError {
    ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "invalid_cursor",
        message: "The history cursor is invalid or ahead of this run.".into(),
    }
}

#[cfg(test)]
mod tests;
