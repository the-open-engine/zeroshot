use super::*;
use super::super::lineage::{successor, suspended};
use std::collections::BTreeMap;

/// The discovered archive always serves physical history. A hosted summary can exist
/// before its ledger is readable. Small menu pages exercise real cursor boundaries.
struct Archive {
    history: NativeRunHistory,
    rows: Mutex<BTreeMap<RunId, RunHistorySummary>>,
    list_reads: tokio::sync::broadcast::Sender<Option<RunId>>,
}

impl Archive {
    async fn serve(self: &Arc<Self>) -> LocalServer {
        local_server_with_runs(NativeRunHistory::transported(self.clone())).await
    }

    async fn from_fixture(fixture: &Fixture) -> Arc<Self> {
        let list = fixture.service.physical_list(None).await.assert_value();
        Arc::new(Self {
            history: fixture.service.clone(),
            rows: Mutex::new(
                list.runs
                    .into_iter()
                    .map(|row| (row.run_id.clone(), row))
                    .collect(),
            ),
            list_reads: tokio::sync::broadcast::channel(128).0,
        })
    }

    fn handoff(&self, from: &RunId, to: &RunId) {
        let mut rows = self.rows.lock().assert_value();
        rows.get_mut(from)
            .assert_value()
            .workspace_recovery
            .get_or_insert_default()
            .successor_run_id = Some(to.clone());
        rows.get_mut(to)
            .assert_value()
            .workspace_recovery
            .get_or_insert_default()
            .resumed_from = Some(from.clone());
    }

    fn list(&self, after: Option<RunId>) -> Value {
        let rows = self.rows.lock().assert_value();
        let mut visible = rows
            .values()
            .rev()
            .filter(|row| after.as_ref().is_none_or(|id| &row.run_id < id));
        let page = visible.next();
        let next = visible.next().and(page).map(|row| &row.run_id);
        let _ = self.list_reads.send(after);
        json!({"runs":page.into_iter().collect::<Vec<_>>(),"nextCursor":next})
    }
}

#[async_trait::async_trait]
impl RunHistoryTransport for Archive {
    async fn get(
        &self,
        request: RunHistoryRequest,
    ) -> Result<RunHistoryResponse, RunHistoryTransportError> {
        let (status, body) = match request {
            RunHistoryRequest::List { after } => (200, self.list(after)),
            RunHistoryRequest::Detail { ref id } | RunHistoryRequest::Page { ref id, .. } => {
                let row = self.rows.lock().assert_value().get(id).cloned();
                match row {
                    None => (
                        404,
                        json!({"code":"run_not_found","message":"Unknown run."}),
                    ),
                    Some(row) if !row.history_available => (
                        503,
                        json!({"code":"history_pending","message":"Preparing history."}),
                    ),
                    Some(row) => match request {
                        RunHistoryRequest::Page { id, after } => (
                            200,
                            self.history.page(&id, Some(after)).await.assert_value(),
                        ),
                        _ => {
                            let mut detail = self.history.detail(id).await.assert_value();
                            detail["workspaceRecovery"] = json!(row.workspace_recovery);
                            (200, detail)
                        }
                    },
                }
            }
        };
        RunHistoryResponse::new(status, serde_json::to_vec(&body).assert_value())
    }
}

async fn finished_part(fixture: &Fixture) -> RunId {
    let id = RunId::new(uuid::Uuid::now_v7().to_string());
    let ledger = successor(fixture, &id).await;
    ledger
        .append(&id, vec![RunEvent::RunStarted, live::terminal()])
        .await
        .assert_value();
    id
}

async fn suspended_with_child() -> (Fixture, RunId) {
    let fixture = Fixture::new().await;
    suspended(&fixture).await;
    let child = finished_part(&fixture).await;
    (fixture, child)
}

async fn menu(server: &LocalServer) -> Vec<Value> {
    let (status, body) = read(server, "").await;
    assert_eq!(status, StatusCode::OK);
    body["runs"].as_array().assert_value().clone()
}

async fn read(server: &LocalServer, path: &str) -> (StatusCode, Value) {
    let response = ui_get(
        &reqwest::Client::new(),
        server,
        &format!("/ui/api/runs{path}"),
    )
    .send()
    .await
    .assert_value();
    (response.status(), response.json().await.assert_value())
}

#[tokio::test]
async fn queued_successor_is_one_run_with_readable_predecessor_history() {
    let (fixture, child) = suspended_with_child().await;
    let root = &fixture.id;
    let archive = Archive::from_fixture(&fixture).await;
    archive.handoff(root, &child);
    {
        let mut rows = archive.rows.lock().assert_value();
        let child = rows.get_mut(&child).assert_value();
        child.phase = RunHistoryPhase::Queued;
        child.history_available = false;
        child.cursor = None;
        child.terminal = None;
    }
    let server = archive.serve().await;
    let rows = menu(&server).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row["runId"], json!(root));
    assert_eq!(row["currentPartId"], json!(child));
    assert_eq!(row["phase"], "queued");
    let mut reads = archive.list_reads.subscribe();
    let (status, history) = read(&server, &format!("/{}/history", root.as_str())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["events"].as_array().assert_value().len(), 3);
    assert_eq!(history["nextCursor"], format!("{}@v2:0", child.as_str()));
    assert_eq!(history["headCursor"], history["nextCursor"]);
    assert_eq!(reads.try_recv().assert_value(), None);
    assert!(matches!(
        reads.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    let (status, exact) = read(
        &server,
        &format!("/{}?partId={}", child.as_str(), root.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(exact["currentPartId"], json!(child));
    assert_eq!(exact["parts"], json!([root, child]));
    assert_eq!(exact["partView"], true);
    assert_eq!(exact["phase"], "suspended");
    for suffix in [
        "".to_owned(),
        format!("/events?after={}@v2:0", child.as_str()),
    ] {
        let (status, problem) = read(&server, &format!("/{}{suffix}", root.as_str())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(problem["code"], "history_pending");
    }
}

#[tokio::test]
async fn lost_remote_history_ends_the_follow_with_one_typed_problem() {
    let fixture = Fixture::new().await;
    suspended(&fixture).await;
    let archive = Archive::from_fixture(&fixture).await;
    let server = archive.serve().await;
    let path = format!("/ui/api/runs/{}/events?after=v2:3", fixture.id.as_str());
    let mut response = ui_get(&reqwest::Client::new(), &server, &path)
        .send()
        .await
        .assert_value();
    assert_eq!(response.status(), StatusCode::OK);
    let first = response.chunk().await.assert_value().assert_value();
    assert!(String::from_utf8_lossy(&first).contains("event: history\n"));
    archive.rows.lock().assert_value().clear();
    let remaining = tokio::time::timeout(Duration::from_secs(5), response.text())
        .await
        .assert_value()
        .assert_value();
    assert_eq!(
        remaining.matches("event: history_error").count(),
        1,
        "{remaining}"
    );
    assert!(remaining.contains("\"code\":\"run_not_found\""));
}

#[tokio::test]
async fn suspended_remote_follow_reuses_its_menu_page_and_reconnects_across_the_handoff() {
    let (fixture, child) = suspended_with_child().await;
    let root = &fixture.id;
    let archive = Archive::from_fixture(&fixture).await;
    let server = archive.serve().await;
    let client = reqwest::Client::new();
    let events = format!("/ui/api/runs/{}/events", root.as_str());
    let mut reads = archive.list_reads.subscribe();
    let mut response = ui_get(&client, &server, &format!("{events}?after=v2:0"))
        .header("last-event-id", format!("{}@v2:3", root.as_str()))
        .send()
        .await
        .assert_value();
    let first = response.chunk().await.assert_value().assert_value();
    let first = String::from_utf8_lossy(&first);
    assert!(first.contains(&format!("id: {}@v2:3", root.as_str())));
    assert!(first.contains("\"events\":[]"));
    // The suspended part sits below the newest page. The second idle probe resumes
    // immediately above it without fetching newer pages or reloading its definition.
    for expected in [None, Some(child.clone()), Some(child.clone())] {
        let after = tokio::time::timeout(Duration::from_secs(4), reads.recv())
            .await
            .assert_value()
            .assert_value();
        assert_eq!(after, expected);
    }
    archive.handoff(root, &child);
    let follow = tokio::time::timeout(Duration::from_secs(8), response.text())
        .await
        .assert_value()
        .assert_value();
    assert!(
        follow.contains(&format!("id: {}@v2:0", child.as_str())),
        "{follow}"
    );
    assert!(
        follow.contains(&format!("id: {}@v2:2", child.as_str())),
        "{follow}"
    );
    assert!(follow.contains("\"finished\":true"));

    let response = ui_get(
        &client,
        &server,
        &format!("{events}?partId={}&after=v2:0", root.as_str()),
    )
    .header("last-event-id", format!("{}@v2:2", root.as_str()))
    .send()
    .await
    .assert_value();
    assert_eq!(response.status(), StatusCode::OK);
    let exact = response.text().await.assert_value();
    assert!(exact.contains("id: v2:3"));
    assert!(exact.contains("\"partView\":true"));
    assert!(exact.contains("\"kind\":\"suspended\""));
    assert!(!exact.contains("id: v2:2"));
}

#[tokio::test]
async fn remote_browser_requires_reciprocal_links_and_refuses_foreign_replay_positions() {
    let (fixture, middle) = suspended_with_child().await;
    let root = &fixture.id;
    let last = finished_part(&fixture).await;
    let archive = Archive::from_fixture(&fixture).await;
    archive.handoff(root, &middle);
    archive
        .rows
        .lock()
        .assert_value()
        .get_mut(&middle)
        .assert_value()
        .workspace_recovery
        .as_mut()
        .assert_value()
        .successor_run_id = Some(last.clone());
    let server = archive.serve().await;
    let rows = menu(&server).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["currentPartId"], json!(middle));
    let run = format!("/{}", middle.as_str());
    let cases = [
        (
            format!("{run}?after=v2:0"),
            StatusCode::BAD_REQUEST,
            "invalid_cursor",
        ),
        (
            format!("{run}?partId={}", last.as_str()),
            StatusCode::NOT_FOUND,
            "run_not_found",
        ),
        (
            format!(
                "{run}/history?partId={}&after={}@v2:0",
                root.as_str(),
                middle.as_str()
            ),
            StatusCode::BAD_REQUEST,
            "invalid_cursor",
        ),
        (
            format!("{run}/events?partId={}&after=v2:01", root.as_str()),
            StatusCode::BAD_REQUEST,
            "invalid_cursor",
        ),
        (
            format!("{run}/history?after={}@v2:1", last.as_str()),
            StatusCode::BAD_REQUEST,
            "invalid_cursor",
        ),
        (
            format!("{run}/events?after=v2:0&after=v2:1"),
            StatusCode::BAD_REQUEST,
            "invalid_cursor",
        ),
    ];
    for (path, expected, code) in cases {
        let (status, body) = read(&server, &path).await;
        assert_eq!(status, expected, "{path}: {body}");
        assert_eq!(body["code"], code, "{path}");
    }
    archive.handoff(&middle, &last);
    let (status, detail) = read(&server, &run).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["parts"], json!([root, middle, last]));
    let (_, page) = read(&server, &format!("{run}/history?after=v2:1")).await;
    assert_eq!(page["partId"], json!(middle));
    assert_eq!(page["events"].as_array().assert_value().len(), 1);
    assert_eq!(page["nextCursor"], format!("{}@v2:0", last.as_str()));
    archive.handoff(&last, root);
    let (status, problem) = read(&server, &run).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(problem["code"], "history_unavailable");
}
