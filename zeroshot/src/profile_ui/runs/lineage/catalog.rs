//! Remote archives expose compact menu pages and separate full definitions. Join menu
//! entries on committed links without downloading every graph or loading an entire account.
use super::*;

pub(super) struct RemoteCatalog {
    source: RemoteRunHistory,
    linked: HashMap<RunId, RunHistorySummary>,
    after: Option<RunId>,
    exhausted: bool,
}

impl RemoteCatalog {
    pub(super) fn new(source: RemoteRunHistory) -> Self {
        Self {
            source,
            linked: HashMap::new(),
            after: None,
            exhausted: false,
        }
    }

    pub(super) fn remember(&mut self, after: Option<&RunId>, page: &RunHistoryList) {
        for entry in &page.runs {
            if entry.history_available && has_links(entry.workspace_recovery.as_ref()) {
                self.linked.insert(entry.run_id.clone(), entry.clone());
            }
        }
        if self.after.as_ref() == after {
            self.after = page.next_cursor.clone();
            self.exhausted = self.after.is_none();
        }
    }

    async fn linked(&mut self, id: &RunId) -> Result<Option<&RunHistorySummary>, ApiError> {
        while !self.linked.contains_key(id)
            && !self.exhausted
            && self.after.as_ref().is_none_or(|last| id < last)
        {
            let after = self.after.clone();
            let page = self.source.list(after.as_ref()).await?;
            self.remember(after.as_ref(), &page);
        }
        Ok(self.linked.get(id))
    }
}

pub(super) fn has_links(recovery: Option<&WorkspaceRecovery>) -> bool {
    recovery
        .is_some_and(|record| record.resumed_from.is_some() || record.successor_run_id.is_some())
}

pub(super) type Catalog = tokio::sync::Mutex<RemoteCatalog>;

pub(super) async fn resolve(
    source: &NativeRunHistory,
    id: &RunId,
    origin: Option<WorkspaceRecovery>,
    catalog: Option<&Catalog>,
) -> Result<RunLineage, ApiError> {
    RunLineage::resolve(id, |part| {
        let origin = (id == &part).then(|| origin.clone()).flatten();
        async move {
            if origin.is_some() {
                return Ok(origin);
            }
            if let Some(remote) = catalog {
                Ok(remote
                    .lock()
                    .await
                    .linked(&part)
                    .await?
                    .and_then(|entry| entry.workspace_recovery.clone()))
            } else {
                source.recovery(part).await
            }
        }
    })
    .await
    .map_err(lineage_error)
}

pub(super) async fn head_summary(
    source: &NativeRunHistory,
    id: &RunId,
    catalog: Option<&Catalog>,
) -> Result<RunHistorySummary, ApiError> {
    if let Some(remote) = catalog
        && let Some(head) = remote.lock().await.linked(id).await?.cloned()
    {
        return Ok(head);
    }
    source.part_summary(id).await
}

/// The v1 archive has no single-row summary route. Remember the menu page immediately
/// above a suspended part and refresh that bounded page while waiting for a successor.
#[derive(Default)]
pub(super) struct HeadProbe {
    after: Option<RunId>,
}

impl HeadProbe {
    pub(super) async fn read(
        &mut self,
        source: &NativeRunHistory,
        id: &RunId,
    ) -> Result<RunHistorySummary, ApiError> {
        let NativeRunHistorySource::Remote(remote) = &source.0 else {
            return source.part_summary(id).await;
        };
        let mut after = self.after.clone().filter(|after| id < after);
        loop {
            let page = remote.list(after.as_ref()).await?;
            if let Some(entry) = page.runs.into_iter().find(|entry| &entry.run_id == id) {
                self.after = after;
                return Ok(entry);
            }
            match page.next_cursor {
                Some(next) if id < &next => after = Some(next),
                _ => return source.part_summary(id).await,
            }
        }
    }
}

/// During a physical listing the newest part appears before its original submission.
/// Defer that family until its root is reached, even across a physical page boundary.
#[derive(Default)]
pub(super) struct MenuFamilies {
    roots: HashMap<RunId, RunLineage>,
    members: HashSet<RunId>,
}

impl MenuFamilies {
    pub(super) async fn head(
        &mut self,
        source: &NativeRunHistory,
        entry: &RunHistorySummary,
        catalog: Option<&Catalog>,
    ) -> Result<Option<(RunId, usize)>, ApiError> {
        let id = &entry.run_id;
        if self.members.contains(id) {
            return Ok(None);
        }
        let family = match self.roots.remove(id) {
            Some(family) => family,
            None if source.needs_lineage_lookup(entry) => {
                let origin = catalog
                    .and(entry.workspace_recovery.clone())
                    .filter(|_| entry.history_available);
                resolve(source, id, origin, catalog).await?
            }
            None => return Ok(Some((id.clone(), 1))),
        };
        self.members.extend(family.parts().iter().skip(1).cloned());
        if family.root() == id {
            Ok(Some((family.current().clone(), family.parts().len())))
        } else {
            self.roots.insert(family.root().clone(), family);
            Ok(None)
        }
    }
}

pub(super) fn menu_row(
    entry: RunHistorySummary,
    id: &RunId,
    current: &RunId,
    count: usize,
) -> Result<Value, ApiError> {
    let mut row = serde_json::to_value(entry).map_err(|_| unavailable())?;
    row["runId"] = json!(id);
    row["createdAt"] = json!(created_at(id));
    row["currentPartId"] = json!(current);
    row["partId"] = json!(current);
    row["partCount"] = json!(count);
    if let Some(cursor) = row["cursor"].as_str() {
        row["cursor"] = json!(part_cursor(current, &Cursor::new(cursor)));
    }
    qualify_state(&mut row, current);
    Ok(row)
}
