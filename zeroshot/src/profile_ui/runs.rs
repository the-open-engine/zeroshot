//! Read-only history services. The HTTP adapter only selects a retained run and a replay cursor;
//! immutable definitions and events come from the same native ledger used by the CLI.
use std::convert::Infallible;
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::{stream, Stream, StreamExt};
use openengine_cluster_protocol::{is_canonical_uuid_v7, Cursor, RunId, RunTitle};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{ApiError, RunHistoryRequest, RunHistoryTransport, RunHistoryTransportError, UiState};
use crate::native_v2_cli::{default_local_state_root, NativeV2CliError};
use crate::v2_run_ledger::sqlite::SqliteRunLedger;
use crate::v2_run_ledger::{
    cursor_sequence, initial_cursor, RunLedger, RunLedgerError, RunSnapshot, StoredRun,
};

use crate::native_v2_observability::history::{
    control, created_at, status, validate_history_page, validate_run_definition, HistoryError,
    HistoryPage, HistoryProblemCode, ObservationState, RunDefinition, RunHistoryList,
    RunHistoryPhase, RunHistoryService, RunHistorySummary, RunHistoryTerminalSynopsis,
    RUN_HISTORY_LIST_PAGE_SIZE,
};
use control::ControlCache;
use status::{RuntimeFailure, RuntimeStatusReader};

impl From<HistoryError> for ApiError {
    fn from(error: HistoryError) -> Self {
        Self {
            status: StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            code: error.code.as_str(),
            message: error.message,
        }
    }
}

const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const REMOTE_LIVE_POLL_MAX_INTERVAL: Duration = Duration::from_secs(4);

#[derive(Clone)]
enum LiveHistorySource {
    Native(RunHistoryService),
    Remote(RemoteRunHistory),
}

impl LiveHistorySource {
    async fn page(&self, id: &RunId, after: Cursor) -> Result<HistoryPage, ApiError> {
        match self {
            Self::Native(service) => service.page(id, Some(after)).await.map_err(Into::into),
            Self::Remote(service) => service.page(id, Some(after)).await,
        }
    }

    fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }
}

struct LiveHistory {
    source: LiveHistorySource,
    id: RunId,
    after: Cursor,
    first: Option<HistoryPage>,
    at_head: bool,
    done: bool,
    unavailable: bool,
    last_page_state: Option<LivePageState>,
    poll_interval: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LivePageState {
    observation: ObservationState,
    observation_code: Option<String>,
    finished: bool,
    runtime_failure: Option<RuntimeFailure>,
    control_error: Option<String>,
}

impl LivePageState {
    fn from_page(page: &HistoryPage) -> Self {
        Self {
            observation: page.observation.state,
            observation_code: page.observation.code.clone(),
            finished: page.finished,
            runtime_failure: page.runtime_failure.clone(),
            control_error: page.control_error.clone(),
        }
    }

    fn can_remain_idle(&self) -> bool {
        matches!(
            self.observation,
            ObservationState::Active | ObservationState::Collecting
        )
    }
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
            let page = match self.poll_page().await {
                Ok(Some(page)) => page,
                Ok(None) => continue,
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            };
            self.last_page_state = Some(LivePageState::from_page(&page));
            self.after = page.next_cursor.clone();
            self.at_head = page.complete;
            self.done = page.complete && page.observation.state == ObservationState::Complete;
            self.unavailable =
                page.complete && page.observation.state == ObservationState::Incomplete;
            return Some(Ok(page));
        }
    }

    async fn poll_page(&mut self) -> Result<Option<HistoryPage>, ApiError> {
        if let Some(page) = self.first.take() {
            return Ok(Some(page));
        }
        if self.at_head {
            tokio::time::sleep(self.poll_interval).await;
        }
        let page = self.source.page(&self.id, self.after.clone()).await?;
        let page_state = LivePageState::from_page(&page);
        if page.events.is_empty()
            && page_state.can_remain_idle()
            && self.last_page_state.as_ref() == Some(&page_state)
        {
            self.record_idle_poll();
            return Ok(None);
        }
        self.poll_interval = LIVE_POLL_INTERVAL;
        Ok(Some(page))
    }

    fn record_idle_poll(&mut self) {
        if self.source.is_remote() {
            self.poll_interval = self
                .poll_interval
                .saturating_mul(2)
                .min(REMOTE_LIVE_POLL_MAX_INTERVAL);
        }
    }
}

#[derive(Clone)]
pub(super) struct NativeRunHistory(NativeRunHistorySource);

#[derive(Clone)]
enum NativeRunHistorySource {
    Local(LocalRunHistory),
    Remote(RemoteRunHistory),
}

#[derive(Clone)]
struct LocalRunHistory {
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
        Ok(Self(NativeRunHistorySource::Local(LocalRunHistory {
            status: Some(RuntimeStatusReader::Local(root.clone())),
            root,
            layout: LedgerLayout::Local,
            control: ControlCache::default(),
        })))
    }

    #[cfg(test)]
    pub(super) fn new(root: PathBuf) -> Self {
        Self(NativeRunHistorySource::Local(LocalRunHistory {
            status: None,
            root,
            layout: LedgerLayout::Local,
            control: ControlCache::default(),
        }))
    }

    pub(super) fn target(
        root: PathBuf,
        observations: crate::native_v2_observability::NativeV2Observability,
    ) -> Self {
        Self(NativeRunHistorySource::Local(LocalRunHistory {
            root,
            layout: LedgerLayout::Target,
            control: ControlCache::default(),
            status: Some(RuntimeStatusReader::Target(observations)),
        }))
    }

    pub(super) fn transported(transport: Arc<dyn RunHistoryTransport>) -> Self {
        Self(NativeRunHistorySource::Remote(RemoteRunHistory::new(
            transport,
        )))
    }

    #[cfg(test)]
    fn local_for_test(&self) -> &LocalRunHistory {
        let NativeRunHistorySource::Local(local) = &self.0 else {
            panic!("test requires local run history")
        };
        local
    }

    #[cfg(test)]
    fn local_for_test_mut(&mut self) -> &mut LocalRunHistory {
        let NativeRunHistorySource::Local(local) = &mut self.0 else {
            panic!("test requires local run history")
        };
        local
    }

    #[cfg(test)]
    async fn open(&self, id: &RunId) -> Result<SqliteRunLedger, ApiError> {
        self.local_for_test().open(id).await
    }

    async fn list(&self, after: Option<RunId>) -> Result<Value, ApiError> {
        let list = match &self.0 {
            NativeRunHistorySource::Local(local) => local.list(after).await,
            NativeRunHistorySource::Remote(remote) => remote.list(after.as_ref()).await,
        }?;
        serde_json::to_value(list).map_err(|_| unavailable())
    }

    async fn detail(&self, id: &RunId) -> Result<Value, ApiError> {
        let definition = match &self.0 {
            NativeRunHistorySource::Local(local) => local
                .service(id)
                .await?
                .definition(id)
                .await
                .map_err(Into::into),
            NativeRunHistorySource::Remote(remote) => remote.detail(id).await,
        }?;
        serde_json::to_value(definition).map_err(|_| unavailable())
    }

    async fn page(&self, id: &RunId, after: Option<Cursor>) -> Result<Value, ApiError> {
        let page = match &self.0 {
            NativeRunHistorySource::Local(local) => local
                .service(id)
                .await?
                .page(id, after)
                .await
                .map_err(Into::into),
            NativeRunHistorySource::Remote(remote) => remote.page(id, after).await,
        }?;
        serde_json::to_value(page).map_err(|_| unavailable())
    }

    async fn subscribe(
        &self,
        id: RunId,
        after: Option<Cursor>,
    ) -> Result<impl Stream<Item = Result<HistoryPage, ApiError>> + Send + 'static + use<>, ApiError>
    {
        let source = match &self.0 {
            NativeRunHistorySource::Local(local) => {
                LiveHistorySource::Native(local.service(&id).await?)
            }
            NativeRunHistorySource::Remote(remote) => LiveHistorySource::Remote(remote.clone()),
        };
        let after = after.unwrap_or_else(initial_cursor);
        let first = source.page(&id, after.clone()).await?;
        let state = LiveHistory {
            source,
            id,
            after,
            first: Some(first),
            at_head: false,
            done: false,
            unavailable: false,
            last_page_state: None,
            poll_interval: LIVE_POLL_INTERVAL,
        };
        // The response owns the reader and timer. Disconnecting drops both; there is no
        // producer task or event queue that can outlive the HTTP subscription.
        Ok(stream::unfold(state, |mut state| async move {
            state.next().await.map(|page| (page, state))
        }))
    }
}

impl LocalRunHistory {
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
                .list_ids_page(after, (RUN_HISTORY_LIST_PAGE_SIZE + 1) as u16)
                .await
                .map_err(ledger_error);
        }
        let directory = self.root.join("runs");
        let after = after.cloned();
        tokio::task::spawn_blocking(move || scan_local_run_ids(&directory, after.as_ref()))
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

    async fn list(&self, after: Option<RunId>) -> Result<RunHistoryList, ApiError> {
        let ids = self.ids(after.as_ref()).await?;
        let mut remaining = ids.into_iter();
        let page = remaining
            .by_ref()
            .take(RUN_HISTORY_LIST_PAGE_SIZE)
            .collect::<Vec<_>>();
        let next = remaining
            .next()
            .is_some()
            .then(|| page.last().cloned())
            .flatten();
        let runs = stream::iter(page)
            .map(|id| async move {
                match self.stored(&id).await {
                    Ok(stored) => {
                        let failure =
                            status::runtime_observation(self.status.as_ref(), &stored.snapshot)
                                .await
                                .ok()
                                .flatten();
                        Ok(summary(&stored.snapshot, failure))
                    }
                    Err(_) => Ok(RunHistorySummary {
                        title: RunTitle::new(id.as_str()).map_err(|_| unavailable())?,
                        created_at: created_at(&id),
                        run_id: id,
                        phase: RunHistoryPhase::Unavailable,
                        cursor: None,
                        terminal: None,
                        source: None,
                        history_available: false,
                        runtime_failure: None,
                    }),
                }
            })
            .buffered(8)
            .collect::<Vec<Result<RunHistorySummary, ApiError>>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RunHistoryList {
            runs,
            next_cursor: next,
        })
    }

    async fn service(&self, id: &RunId) -> Result<RunHistoryService, ApiError> {
        Ok(RunHistoryService::with_sources(
            Arc::new(self.open(id).await?),
            self.control.clone(),
            self.status.clone(),
        ))
    }
}

fn scan_local_run_ids(directory: &FilePath, after: Option<&RunId>) -> Result<Vec<RunId>, ApiError> {
    let Some(entries) = open_run_directory(directory)? else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for entry in entries {
        if let Some(id) = run_directory_id(entry.map_err(file_error)?, after)? {
            ids.push(id);
        }
    }
    ids.sort_by(|a, b| b.as_str().cmp(a.as_str()));
    ids.truncate(RUN_HISTORY_LIST_PAGE_SIZE + 1);
    Ok(ids)
}

fn open_run_directory(directory: &FilePath) -> Result<Option<std::fs::ReadDir>, ApiError> {
    match std::fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => {
            require_directory(directory)?;
            std::fs::read_dir(directory).map(Some).map_err(file_error)
        }
    }
}

fn run_directory_id(
    entry: std::fs::DirEntry,
    after: Option<&RunId>,
) -> Result<Option<RunId>, ApiError> {
    let metadata = entry.file_type().map_err(file_error)?;
    if !metadata.is_dir() || metadata.is_symlink() {
        return Ok(None);
    }
    let name = entry.file_name();
    let Some(name) = name.to_str() else {
        return Ok(None);
    };
    let id = RunId::new(name);
    if !is_canonical_uuid_v7(&id) || after.is_some_and(|after| id.as_str() >= after.as_str()) {
        return Ok(None);
    }
    Ok(Some(id))
}

#[derive(Clone)]
struct RemoteRunHistory {
    transport: Arc<dyn RunHistoryTransport>,
}

impl RemoteRunHistory {
    fn new(transport: Arc<dyn RunHistoryTransport>) -> Self {
        Self { transport }
    }

    async fn list(&self, after: Option<&RunId>) -> Result<RunHistoryList, ApiError> {
        let list = self.get(RunHistoryRequest::list(after.cloned())).await?;
        validate_remote_list(list, after)
    }

    async fn detail(&self, id: &RunId) -> Result<RunDefinition, ApiError> {
        let definition: RunDefinition = self.get(RunHistoryRequest::detail(id.clone())).await?;
        validate_run_definition(&definition, id).map_err(|_| incompatible())?;
        Ok(definition)
    }

    async fn page(&self, id: &RunId, after: Option<Cursor>) -> Result<HistoryPage, ApiError> {
        let requested = after.unwrap_or_else(initial_cursor);
        let page: HistoryPage = self
            .get(RunHistoryRequest::page(id.clone(), requested.clone()))
            .await?;
        validate_history_page(&page, &requested).map_err(|_| incompatible())?;
        Ok(page)
    }

    async fn get<T: DeserializeOwned>(&self, request: RunHistoryRequest) -> Result<T, ApiError> {
        let success_limit = request.maximum_response_bytes();
        let problem_limit = request.maximum_problem_bytes();
        let response = self.transport.get(request).await.map_err(transport_error)?;
        let (status, body) = response.into_parts();
        let status = StatusCode::from_u16(status).map_err(|_| unavailable())?;
        let maximum = if status.is_success() {
            success_limit
        } else {
            problem_limit
        };
        if body.len() > maximum {
            return Err(unavailable());
        }
        if !status.is_success() {
            return Err(remote_problem(status, &body));
        }
        serde_json::from_slice(&body).map_err(|_| incompatible())
    }
}

fn validate_remote_list(
    list: RunHistoryList,
    after: Option<&RunId>,
) -> Result<RunHistoryList, ApiError> {
    if invalid_remote_list_shape(&list)
        || invalid_remote_list_order(&list, after)
        || invalid_remote_next_cursor(&list)
    {
        return Err(incompatible());
    }
    Ok(list)
}

fn invalid_remote_list_shape(list: &RunHistoryList) -> bool {
    list.runs.len() > RUN_HISTORY_LIST_PAGE_SIZE
        || list
            .next_cursor
            .as_ref()
            .is_some_and(|cursor| !is_canonical_uuid_v7(cursor))
        || list.runs.iter().any(invalid_remote_summary)
}

fn invalid_remote_list_order(list: &RunHistoryList, after: Option<&RunId>) -> bool {
    !list
        .runs
        .windows(2)
        .all(|pair| pair[0].run_id.as_str() > pair[1].run_id.as_str())
        || after.is_some_and(|after| {
            list.runs
                .iter()
                .any(|run| run.run_id.as_str() >= after.as_str())
        })
}

fn invalid_remote_next_cursor(list: &RunHistoryList) -> bool {
    list.next_cursor.as_ref().is_some_and(|cursor| {
        list.runs
            .last()
            .is_none_or(|last| last.run_id.as_str() != cursor.as_str())
    })
}

fn invalid_remote_summary(run: &RunHistorySummary) -> bool {
    if !is_canonical_uuid_v7(&run.run_id)
        || run
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor_sequence(cursor).is_err())
        || run.history_available != run.cursor.is_some()
        || (run.phase == RunHistoryPhase::Unavailable
            && (run.history_available || run.terminal.is_some()))
        || (run.terminal.is_some() && run.phase != RunHistoryPhase::Finished)
    {
        return true;
    }
    invalid_remote_runtime_failure(run)
}

fn invalid_remote_runtime_failure(run: &RunHistorySummary) -> bool {
    match (&run.runtime_failure, &run.terminal) {
        (Some(failure), Some(RunHistoryTerminalSynopsis::Failed { reason })) => {
            run.phase != RunHistoryPhase::Finished || reason != &failure.reason
        }
        (Some(_), _) => true,
        (None, _) => false,
    }
}

fn remote_problem(status: StatusCode, body: &[u8]) -> ApiError {
    let Ok(problem) =
        serde_json::from_slice::<openengine_cluster_protocol::TargetHttpProblem>(body)
    else {
        return unavailable();
    };
    let Some(code) = HistoryProblemCode::parse(problem.code()) else {
        return unavailable();
    };
    ApiError {
        status,
        code: code.as_str(),
        message: problem.message().to_owned(),
    }
}

fn transport_error(error: RunHistoryTransportError) -> ApiError {
    match error {
        RunHistoryTransportError::Incompatible => incompatible(),
        RunHistoryTransportError::Unavailable => unavailable(),
    }
}

fn summary(snapshot: &RunSnapshot, runtime_failure: Option<RuntimeFailure>) -> RunHistorySummary {
    let terminal = runtime_failure
        .as_ref()
        .map(|failure| RunHistoryTerminalSynopsis::Failed {
            reason: failure.reason.clone(),
        })
        .or_else(|| {
            snapshot
                .terminal
                .as_ref()
                .map(RunHistoryTerminalSynopsis::from)
        });
    RunHistorySummary {
        run_id: snapshot.run_id.clone(),
        title: snapshot.title.clone(),
        phase: if runtime_failure.is_some() {
            RunHistoryPhase::Finished
        } else {
            snapshot.phase.clone().into()
        },
        cursor: Some(snapshot.cursor.clone()),
        terminal,
        source: Some(snapshot.source.clone()),
        created_at: created_at(&snapshot.run_id),
        history_available: true,
        runtime_failure,
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
    crate::native_v2_observability::history::ledger_error(error).into()
}

fn not_found() -> ApiError {
    crate::native_v2_observability::history::not_found().into()
}
fn unavailable() -> ApiError {
    crate::native_v2_observability::history::unavailable().into()
}
fn incompatible() -> ApiError {
    ApiError {
        status: StatusCode::BAD_GATEWAY,
        code: HistoryProblemCode::HistoryIncompatible.as_str(),
        message: "The target does not provide compatible run history.".to_owned(),
    }
}
fn runtime_unavailable() -> ApiError {
    crate::native_v2_observability::history::runtime_unavailable().into()
}
fn invalid_cursor() -> ApiError {
    crate::native_v2_observability::history::invalid_cursor().into()
}

#[cfg(test)]
mod tests;
