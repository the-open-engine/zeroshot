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
use openengine_cluster_protocol::{is_canonical_uuid_v7, Cursor, RunId};
use serde::Deserialize;
use std::sync::Arc;
use serde_json::{json, Value};

use super::{ApiError, UiState};
use crate::native_v2_cli::{default_local_state_root, NativeV2CliError};
use crate::v2_run_ledger::sqlite::SqliteRunLedger;
use crate::v2_run_ledger::{
    cursor_sequence, initial_cursor, RunLedger, RunLedgerError, RunSnapshot, StoredRun,
};

use crate::native_v2_observability::history::{
    control, status, created_at, HistoryError, HistoryPage, ObservationState, RunHistoryService,
};
use control::ControlCache;
use status::{RuntimeFailure, RuntimeStatusReader};

impl From<HistoryError> for ApiError {
    fn from(error: HistoryError) -> Self {
        Self {
            status: StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            code: error.code,
            message: error.message,
        }
    }
}

const LIST_PAGE_SIZE: usize = 50;
const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(500);

struct LiveHistory {
    service: RunHistoryService,
    id: RunId,
    after: Cursor,
    first: Option<HistoryPage>,
    at_head: bool,
    done: bool,
    unavailable: bool,
    last_runtime_failure: Option<RuntimeFailure>,
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
                match self.service.page(&self.id, Some(self.after.clone())).await {
                    Ok(page)
                        if page.events.is_empty()
                            && page.observation.state == ObservationState::Active
                            && page.runtime_failure == self.last_runtime_failure =>
                    {
                        continue;
                    }
                    Ok(page) => page,
                    Err(error) => {
                        self.done = true;
                        return Some(Err(error.into()));
                    }
                }
            };
            self.last_runtime_failure = page.runtime_failure.clone();
            self.after = page.next_cursor.clone();
            self.at_head = page.complete;
            self.done = page.complete && page.observation.state == ObservationState::Complete;
            self.unavailable =
                page.complete && page.observation.state == ObservationState::Incomplete;
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
                        if let Ok(Some(failure)) =
                            status::runtime_observation(self.status.as_ref(), &stored.snapshot)
                                .await
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

    async fn service(&self, id: &RunId) -> Result<RunHistoryService, ApiError> {
        Ok(RunHistoryService::with_sources(
            Arc::new(self.open(id).await?),
            self.control.clone(),
            self.status.clone(),
        ))
    }

    async fn detail(&self, id: &RunId) -> Result<Value, ApiError> {
        let definition = self.service(id).await?.definition(id).await?;
        serde_json::to_value(definition).map_err(|_| unavailable())
    }

    async fn page(&self, id: &RunId, after: Option<Cursor>) -> Result<Value, ApiError> {
        let page = self.service(id).await?.page(id, after).await?;
        serde_json::to_value(page).map_err(|_| unavailable())
    }

    async fn subscribe(
        &self,
        id: RunId,
        after: Option<Cursor>,
    ) -> Result<impl Stream<Item = Result<HistoryPage, ApiError>> + Send + 'static + use<>, ApiError>
    {
        let service = self.service(&id).await?;
        let after = after.unwrap_or_else(initial_cursor);
        let first = service.page(&id, Some(after.clone())).await?;
        let state = LiveHistory {
            service,
            id,
            after,
            first: Some(first),
            at_head: false,
            done: false,
            unavailable: false,
            last_runtime_failure: None,
        };
        // The response owns the reader and timer. Disconnecting drops both; there is no
        // producer task or event queue that can outlive the HTTP subscription.
        Ok(stream::unfold(state, |mut state| async move {
            state.next().await.map(|page| (page, state))
        }))
    }
}

fn summary(snapshot: &RunSnapshot) -> Value {
    json!({
        "runId":snapshot.run_id,"title":snapshot.title,"phase":snapshot.phase,
        "cursor":snapshot.cursor,"terminal":snapshot.terminal,"source":snapshot.source,
        "createdAt":created_at(&snapshot.run_id),"historyAvailable":true
    })
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
    crate::native_v2_observability::history::ledger_error(error).into()
}

fn not_found() -> ApiError {
    crate::native_v2_observability::history::not_found().into()
}
fn unavailable() -> ApiError {
    crate::native_v2_observability::history::unavailable().into()
}
fn runtime_unavailable() -> ApiError {
    crate::native_v2_observability::history::runtime_unavailable().into()
}
fn invalid_cursor() -> ApiError {
    crate::native_v2_observability::history::invalid_cursor().into()
}

#[cfg(test)]
mod tests;
