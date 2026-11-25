//! Browser history is grouped by confirmed workspace handoffs. Target archives remain per part.
use super::*;
mod catalog;
use catalog::{Catalog, RemoteCatalog};
use axum::response::{IntoResponse, Response};
use openengine_cluster_protocol::{TokenUsage, WorkspaceRecovery};
use std::collections::{HashMap, HashSet};
use crate::native_v2_observability::lineage::{LineageError, RunLineage, split_cursor};
use crate::v2_run_ledger::merge_usage;
use crate::native_v2_observability::history::parts::{
    grouped_definition, grouped_summary, RunPartsPage,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(in crate::profile_ui) struct BrowserQuery {
    after: Option<String>,
    part_id: Option<String>,
}

struct BrowserFamily {
    chain: RunLineage,
    remote_head: Option<RunHistorySummary>,
}

type BrowserParams = Result<Query<BrowserQuery>, axum::extract::rejection::QueryRejection>;

async fn selection(
    source: &NativeRunHistory,
    id: String,
    query: BrowserParams,
) -> Result<(RunId, BrowserQuery, BrowserFamily), ApiError> {
    let query = query.map_err(|_| invalid_cursor())?.0;
    let id = parse_id(id)?;
    let family = source.family(&id).await?;
    Ok((id, query, family))
}

impl LocalRunHistory {
    async fn part_recovery(&self, id: RunId) -> Result<Option<WorkspaceRecovery>, ApiError> {
        match self.layout {
            LedgerLayout::Local => crate::native_v2_cli::local::LocalCliBackend::new(
                self.root.clone(),
                PathBuf::new(),
                PathBuf::new(),
                PathBuf::new(),
            )
            .part_recovery(id)
            .await
            .map_err(|_| unavailable()),
            LedgerLayout::Target => match &self.status {
                Some(RuntimeStatusReader::Target(observer)) => observer
                    .recovery_for_part(id)
                    .await
                    .map_err(|_| unavailable()),
                _ => Ok(None),
            },
        }
    }
}

impl NativeRunHistory {
    async fn recovery(&self, id: RunId) -> Result<Option<WorkspaceRecovery>, ApiError> {
        if !is_canonical_uuid_v7(&id) {
            return Err(incompatible());
        }
        match &self.0 {
            NativeRunHistorySource::Remote(remote) => match remote.detail(&id).await {
                Ok(detail) => Ok(Some(detail.workspace_recovery.unwrap_or_default())),
                Err(error) if error.status == StatusCode::NOT_FOUND => Ok(None),
                Err(error) => Err(error),
            },
            NativeRunHistorySource::Local(local) => local.part_recovery(id).await,
        }
    }

    async fn family(&self, id: &RunId) -> Result<BrowserFamily, ApiError> {
        let catalog = self.remote_catalog();
        let origin = if let Some(catalog) = &catalog {
            match self.recovery(id.clone()).await {
                Err(error) if error.code == HistoryProblemCode::HistoryPending.as_str() => {
                    // A host can commit a queued handoff before its ledger is readable.
                    catalog
                        .lock()
                        .await
                        .linked(id)
                        .await?
                        .ok_or(error)?
                        .workspace_recovery
                        .clone()
                }
                result => result?,
            }
        } else {
            None
        };
        let chain = catalog::resolve(self, id, origin, catalog.as_ref()).await?;
        let remote_head =
            catalog.and_then(|catalog| catalog.into_inner().into_head(chain.current()));
        Ok(BrowserFamily { chain, remote_head })
    }

    fn remote_catalog(&self) -> Option<Catalog> {
        match &self.0 {
            NativeRunHistorySource::Remote(source) => {
                Some(Catalog::new(RemoteCatalog::new(source.clone())))
            }
            _ => None,
        }
    }

    async fn browser_list(&self, after: Option<RunId>) -> Result<Value, ApiError> {
        let mut page_after = after;
        let catalog = self.remote_catalog();
        let mut runs = Vec::new();
        let mut families = catalog::MenuFamilies::default();
        loop {
            let page = self.physical_list(page_after.clone()).await?;
            if let Some(remote) = &catalog {
                remote.lock().await.remember(page_after.as_ref(), &page);
            }
            for entry in page.runs {
                let id = entry.run_id.clone();
                let Some((current, count)) = families.head(self, &entry, catalog.as_ref()).await?
                else {
                    continue;
                };
                if runs.len() == RUN_HISTORY_LIST_PAGE_SIZE {
                    let next = runs.last().and_then(|row: &Value| row.get("runId"));
                    return Ok(json!({"runs":runs,"nextCursor":next}));
                }
                let entry = if current == id {
                    entry
                } else {
                    catalog::head_summary(self, &current, catalog.as_ref()).await?
                };
                runs.push(grouped_summary(&id, count, entry));
            }
            let Some(cursor) = page.next_cursor else {
                break;
            };
            page_after = Some(cursor);
        }
        Ok(json!({"runs":runs, "nextCursor":null}))
    }

    fn needs_lineage_lookup(&self, entry: &RunHistorySummary) -> bool {
        // Local archive status may be unavailable after the controller exits; ownership is
        // still retained separately and should not depend on probing a controller socket.
        (entry.history_available && matches!(&self.0, NativeRunHistorySource::Local(_)))
            || catalog::has_links(entry.workspace_recovery.as_ref())
    }

    async fn part_summary(&self, id: &RunId) -> Result<RunHistorySummary, ApiError> {
        match &self.0 {
            NativeRunHistorySource::Local(local) => {
                let stored = local.stored(id).await?;
                let failure = status::runtime_observation(local.status.as_ref(), &stored.snapshot)
                    .await
                    .ok()
                    .flatten();
                let mut summary = summary(&stored.snapshot, failure);
                summary.workspace_recovery = local.part_recovery(id.clone()).await?;
                Ok(summary)
            }
            NativeRunHistorySource::Remote(remote) => {
                let detail = remote.detail(id).await?;
                Ok(RunHistorySummary {
                    suspension: detail.suspension,
                    suspend_requested: detail.suspend_requested,
                    workspace_recovery: detail.workspace_recovery,
                    run_id: detail.run_id,
                    title: detail.title,
                    phase: detail.phase.into(),
                    cursor: Some(detail.cursor),
                    terminal: detail.terminal.as_ref().map(Into::into),
                    source: detail.source,
                    context_sources: detail.context_sources,
                    created_at: detail.created_at,
                    history_available: detail.history_available,
                    runtime_failure: detail.runtime_failure,
                })
            }
        }
    }

    async fn usage(&self, id: &RunId) -> Result<Option<TokenUsage>, ApiError> {
        match &self.0 {
            NativeRunHistorySource::Local(local) => {
                Ok(local.stored(id).await?.snapshot.token_usage)
            }
            NativeRunHistorySource::Remote(remote) => {
                Ok(remote.detail(id).await?.metadata.token_usage)
            }
        }
    }

    async fn browser_detail(&self, family: &RunLineage) -> Result<Value, ApiError> {
        let head = family.current();
        let mut detail = self.physical_detail(head).await?;
        let mut total = None;
        for id in family.parts() {
            if id == head {
                break;
            }
            merge_usage(&mut total, self.usage(id).await?.as_ref()).map_err(ledger_error)?;
        }
        merge_usage(&mut total, detail.metadata.token_usage.as_ref()).map_err(ledger_error)?;
        detail.metadata.token_usage = total;
        grouped_definition(family.parts(), json!(detail)).ok_or_else(unavailable)
    }

    async fn browser_page(
        &self,
        family: &BrowserFamily,
        offset: (&RunId, Cursor),
        known_head: Option<RunHistorySummary>,
    ) -> Result<RunPartsPage, ApiError> {
        let (part, cursor) = offset;
        let chain = &family.chain;
        let position = chain
            .parts()
            .iter()
            .position(|id| id == part)
            .ok_or_else(invalid_cursor)?;
        let mut page = self.physical_page(part, Some(cursor.clone())).await?;
        if let Some(previous) = position.checked_sub(1).and_then(|at| chain.parts().get(at)) {
            self.describe_continuation(&mut page, previous, part)
                .await?;
        }
        let at_head = part == chain.current();
        let fetched;
        let current = if at_head && !page.finished {
            None
        } else if let Some(head) = known_head
            .as_ref()
            .or(family.remote_head.as_ref().filter(|_| !at_head))
        {
            Some(head)
        } else {
            fetched = self.part_summary(chain.current()).await?;
            Some(&fetched)
        };
        RunPartsPage::from_part(chain.parts(), (position, cursor), page, current)
            .ok_or_else(invalid_cursor)
    }
    async fn describe_continuation(
        &self,
        page: &mut HistoryPage,
        previous: &RunId,
        part: &RunId,
    ) -> Result<(), ApiError> {
        let Some(envelope) = page.events.iter_mut().find(|envelope| {
            let vector = &envelope["event"]["vector"];
            vector["kind"] == "continuation"
                && vector["runId"] == part.as_str()
                && vector["resumedFrom"] == previous.as_str()
        }) else {
            return Ok(());
        };
        let vector = &envelope["event"]["vector"];
        let reason = if vector.get("checkpointId").and_then(Value::as_str).is_some() {
            "checkpoint"
        } else {
            let previous = self.part_summary(previous).await?;
            match previous.suspension {
                Some(openengine_cluster_protocol::RunSuspension::Wait { .. }) => "wait",
                Some(openengine_cluster_protocol::RunSuspension::Request { .. }) => "request",
                Some(openengine_cluster_protocol::RunSuspension::Command { .. }) => "command",
                None if previous.phase == RunHistoryPhase::Finished => "retry",
                None => return Ok(()),
            }
        };
        envelope["resumeReason"] = json!(reason);
        Ok(())
    }
}

fn lineage_error(error: LineageError<ApiError>) -> ApiError {
    match error {
        LineageError::Read(error) => error,
        LineageError::Cycle => unavailable(),
    }
}

fn exact_part(family: &RunLineage, part: String) -> Result<RunId, ApiError> {
    let part = parse_id(part)?;
    family.contains(&part).then_some(part).ok_or_else(not_found)
}

fn exact_cursor(part: &RunId, after: Option<String>) -> Result<Option<Cursor>, ApiError> {
    after
        .map(|value| {
            let supplied = Cursor::new(value);
            let cursor = match split_cursor(&supplied) {
                Some((owner, local)) if owner == *part => local,
                Some(_) => return Err(invalid_cursor()),
                None => supplied,
            };
            let sequence = cursor_sequence(&cursor).map_err(ledger_error)?;
            if crate::v2_run_ledger::cursor_for(sequence) != cursor {
                return Err(invalid_cursor());
            }
            Ok(cursor)
        })
        .transpose()
}

fn position(
    requested: &RunId,
    family: &RunLineage,
    after: Option<String>,
) -> Result<(RunId, Cursor), ApiError> {
    let Some(value) = after else {
        return Ok((family.root().clone(), initial_cursor()));
    };
    let supplied = Cursor::new(value);
    let (part, cursor) = split_cursor(&supplied).unwrap_or_else(|| (requested.clone(), supplied));
    let sequence = cursor_sequence(&cursor).map_err(ledger_error)?;
    if !family.contains(&part) || crate::v2_run_ledger::cursor_for(sequence) != cursor {
        return Err(invalid_cursor());
    }
    Ok((part, cursor))
}

pub(in crate::profile_ui) async fn list(
    State(state): State<UiState>,
    query: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let after = query
        .map_err(|_| invalid_cursor())?
        .after
        .clone()
        .map(parse_id)
        .transpose()?;
    state.runs.browser_list(after).await.map(Json)
}

fn exact_value(mut value: Value, family: &RunLineage, part: &RunId) -> Value {
    value["runId"] = json!(family.root());
    value["partId"] = json!(part);
    value["currentPartId"] = json!(family.current());
    value["partView"] = json!(true);
    value
}

pub(in crate::profile_ui) async fn show(
    State(state): State<UiState>,
    Path(id): Path<String>,
    query: BrowserParams,
) -> Result<Json<Value>, ApiError> {
    let (_, query, family) = selection(&state.runs, id, query).await?;
    if query.after.is_some() {
        return Err(invalid_cursor());
    }
    match query.part_id {
        Some(part) => {
            let part = exact_part(&family.chain, part)?;
            let mut value = exact_value(state.runs.detail(&part).await?, &family.chain, &part);
            value["parts"] = json!(family.chain.parts());
            Ok(Json(value))
        }
        None => state.runs.browser_detail(&family.chain).await.map(Json),
    }
}

pub(in crate::profile_ui) async fn history(
    State(state): State<UiState>,
    Path(id): Path<String>,
    query: BrowserParams,
) -> Result<Json<Value>, ApiError> {
    let (id, query, family) = selection(&state.runs, id, query).await?;
    if let Some(part) = query.part_id {
        let part = exact_part(&family.chain, part)?;
        let page = state
            .runs
            .page(&part, exact_cursor(&part, query.after)?)
            .await?;
        return Ok(Json(exact_value(page, &family.chain, &part)));
    }
    let (part, cursor) = position(&id, &family.chain, query.after)?;
    let page = state
        .runs
        .browser_page(&family, (&part, cursor), None)
        .await?;
    Ok(Json(page.value()))
}

pub(in crate::profile_ui) async fn events(
    State(state): State<UiState>,
    Path(id): Path<String>,
    query: BrowserParams,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (id, query, family) = selection(&state.runs, id, query).await?;
    let after = cursor_header_or_query(query.after, &headers)?;
    if let Some(part) = query.part_id {
        let part = exact_part(&family.chain, part)?;
        let cursor = exact_cursor(&part, after)?;
        return part_events(state, part, cursor, family.chain).await;
    }
    let (part, cursor) = position(&id, &family.chain, after)?;
    let first = state
        .runs
        .browser_page(&family, (&part, cursor), None)
        .await?;
    let shutdown = state.shutdown.clone();
    let follow = BrowserFollow {
        source: state.runs,
        family,
        first: Some(first),
        part,
        cursor: initial_cursor(),
        last: None,
        at_head: false,
        finished: false,
        unavailable: false,
        interval: LIVE_POLL_INTERVAL,
        head_probe: catalog::HeadProbe::default(),
    };
    let stream = stream::unfold(follow, |mut follow| async move {
        follow.next().await.map(|item| (item, follow))
    })
    .take_until(async move { shutdown.cancelled().await });
    Ok(Sse::new(stream.map(|result| {
        Ok::<_, Infallible>(match result {
            Ok(page) => history_event(&page.history().next_cursor, &page.value()),
            Err(error) => history_error_event(error),
        })
    }))
    .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
    .into_response())
}

async fn part_events(
    state: UiState,
    part: RunId,
    cursor: Option<Cursor>,
    family: RunLineage,
) -> Result<Response, ApiError> {
    let stream = state
        .runs
        .subscribe(part.clone(), cursor)
        .await?
        .take_until(async move { state.shutdown.cancelled().await });
    Ok(Sse::new(stream.map(move |result| {
        Ok::<_, Infallible>(match result {
            Ok(page) => history_event(&page.next_cursor, &exact_value(json!(page), &family, &part)),
            Err(error) => history_error_event(error),
        })
    }))
    .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
    .into_response())
}

struct BrowserFollow {
    source: NativeRunHistory,
    family: BrowserFamily,
    first: Option<RunPartsPage>,
    part: RunId,
    cursor: Cursor,
    last: Option<(Cursor, LivePageState)>,
    at_head: bool,
    finished: bool,
    unavailable: bool,
    interval: Duration,
    head_probe: catalog::HeadProbe,
}

impl BrowserFollow {
    async fn next(&mut self) -> Option<Result<RunPartsPage, ApiError>> {
        if self.finished {
            return None;
        }
        if self.unavailable {
            self.finished = true;
            return Some(Err(runtime_unavailable()));
        }
        loop {
            let page = match self.poll().await {
                Ok(page) => page,
                Err(error) => {
                    self.finished = true;
                    return Some(Err(error));
                }
            };
            let history = page.history();
            self.observe_lifecycle(history);
            let state = (
                history.next_cursor.clone(),
                LivePageState::from_page(history),
            );
            if history.events.is_empty()
                && self.last.as_ref() == Some(&state)
                && state.1.can_remain_idle()
            {
                if matches!(&self.source.0, NativeRunHistorySource::Remote(_)) {
                    self.interval = self
                        .interval
                        .saturating_mul(2)
                        .min(REMOTE_LIVE_POLL_MAX_INTERVAL);
                }
                continue;
            }
            self.interval = LIVE_POLL_INTERVAL;
            self.last = Some(state);
            (self.part, self.cursor) = split_cursor(&history.next_cursor)?;
            return Some(Ok(page));
        }
    }

    fn observe_lifecycle(&mut self, page: &HistoryPage) {
        self.at_head = page.complete;
        self.finished =
            self.at_head && page.finished && page.observation.state == ObservationState::Complete;
        self.unavailable = self.at_head && page.observation.state == ObservationState::Incomplete;
    }

    async fn poll(&mut self) -> Result<RunPartsPage, ApiError> {
        if let Some(first) = self.first.take() {
            return Ok(first);
        }
        let known_head = self.refresh_head().await?;
        let page = self
            .source
            .browser_page(&self.family, (&self.part, self.cursor.clone()), known_head)
            .await?;
        if page.history().complete && page.history().finished {
            // A failed part may already have a confirmed retry by the time a slow reader
            // drains its last page. Recheck ownership before closing that stream.
            let current = self.source.family(&self.part).await?;
            if current.chain.current() != self.family.chain.current() {
                self.family = current;
                return self
                    .source
                    .browser_page(&self.family, (&self.part, self.cursor.clone()), None)
                    .await;
            }
        }
        Ok(page)
    }
    async fn refresh_head(&mut self) -> Result<Option<RunHistorySummary>, ApiError> {
        if !self.at_head {
            return Ok(None);
        }
        tokio::time::sleep(self.interval).await;
        if !self
            .last
            .as_ref()
            .is_some_and(|(_, state)| state.observation == ObservationState::Collecting)
        {
            return Ok(None);
        }
        // A suspension may last days. Only re-read ancestry after its owner reports a handoff.
        let head = self.head_probe.read(&self.source, &self.part).await?;
        if head
            .workspace_recovery
            .as_ref()
            .is_some_and(|r| r.successor_run_id.is_some())
        {
            self.family = self.source.family(&self.part).await?;
        }
        Ok((self.family.chain.current() == &self.part).then_some(head))
    }
}
