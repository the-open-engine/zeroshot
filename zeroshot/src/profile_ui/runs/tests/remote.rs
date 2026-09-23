use super::*;
use axum::http::header;
use reqwest::Url;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;

use crate::profile_ui::RunHistoryResponse;

struct TargetServer {
    origin: String,
    service: crate::profile_ui::UiService,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TargetServer {
    fn drop(&mut self) {
        self.service.shutdown();
        self.task.abort();
    }
}

async fn target_server(fixture: &Fixture) -> TargetServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .assert_value();
    let address = listener.local_addr().assert_value();
    let origin = format!("http://{address}");
    let service = crate::profile_ui::UiService::for_target(
        fixture.root.join("runs").join(fixture.id.as_str()),
        &origin,
        fixture.observations(),
    )
    .assert_value();
    let serving = service.clone();
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.assert_value();
            let service = serving.clone();
            tokio::spawn(async move {
                service.serve_connection(stream).await.assert_value();
            });
        }
    });
    TargetServer {
        origin,
        service,
        task,
    }
}

struct LocalServer {
    origin: String,
    root: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn local_server(target_origin: &str) -> LocalServer {
    let transport = HttpRunHistoryTransport::new(target_origin);
    local_server_with_runs(NativeRunHistory::transported(Arc::new(transport))).await
}

async fn local_server_with_runs(runs: NativeRunHistory) -> LocalServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .assert_value();
    let address = listener.local_addr().assert_value();
    let origin = format!("http://{address}");
    let root =
        std::env::temp_dir().join(format!("zeroshot-remote-ui-test-{}", uuid::Uuid::now_v7()));
    let state = UiState::new(
        crate::native_v2_cli::LocalRunProfileStore::new(root.clone()),
        runs,
        &origin,
        "local",
    )
    .assert_value();
    let app = crate::profile_ui::router(state, false);
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.assert_value();
    });
    LocalServer { origin, root, task }
}

#[derive(Clone)]
struct FixedTransport {
    status: u16,
    body: Arc<Vec<u8>>,
    routes: Arc<Mutex<Vec<String>>>,
}

struct HttpRunHistoryTransport {
    origin: Url,
    client: reqwest::Client,
}

#[derive(Clone)]
struct ScriptedTransport {
    bodies: Arc<Mutex<VecDeque<Vec<u8>>>>,
    calls: Arc<Mutex<Vec<tokio::time::Instant>>>,
}

impl HttpRunHistoryTransport {
    fn new(origin: &str) -> Self {
        Self {
            origin: Url::parse(&format!("{origin}/")).assert_value(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .assert_value(),
        }
    }

    fn url(&self, request: &RunHistoryRequest) -> Url {
        let (path, after) = match request {
            RunHistoryRequest::List { after } => (
                "native-v2/run-history".to_owned(),
                after.as_ref().map(RunId::as_str),
            ),
            RunHistoryRequest::Detail { id } => {
                (format!("native-v2/run-history/{}", id.as_str()), None)
            }
            RunHistoryRequest::Page { id, after } => (
                format!("native-v2/run-history/{}/page", id.as_str()),
                Some(after.as_str()),
            ),
        };
        let mut url = self.origin.join(&path).assert_value();
        if let Some(after) = after {
            url.query_pairs_mut().append_pair("after", after);
        }
        url
    }
}

#[async_trait::async_trait]
impl RunHistoryTransport for HttpRunHistoryTransport {
    async fn get(
        &self,
        request: RunHistoryRequest,
    ) -> Result<RunHistoryResponse, RunHistoryTransportError> {
        let response = self
            .client
            .get(self.url(&request))
            .send()
            .await
            .map_err(|_| RunHistoryTransportError::unavailable())?;
        RunHistoryResponse::from_http(&request, response).await
    }
}

#[async_trait::async_trait]
impl RunHistoryTransport for FixedTransport {
    async fn get(
        &self,
        request: RunHistoryRequest,
    ) -> Result<RunHistoryResponse, RunHistoryTransportError> {
        let route = match &request {
            RunHistoryRequest::List { after } => format!(
                "list{}",
                after
                    .as_ref()
                    .map(|after| format!("?after={}", after.as_str()))
                    .unwrap_or_default()
            ),
            RunHistoryRequest::Detail { id } => format!("detail:{}", id.as_str()),
            RunHistoryRequest::Page { id, after } => {
                format!("page:{}?after={}", id.as_str(), after.as_str())
            }
        };
        self.routes.lock().assert_value().push(route);
        RunHistoryResponse::new(self.status, self.body.as_ref().clone())
    }
}

#[async_trait::async_trait]
impl RunHistoryTransport for ScriptedTransport {
    async fn get(
        &self,
        _request: RunHistoryRequest,
    ) -> Result<RunHistoryResponse, RunHistoryTransportError> {
        self.calls
            .lock()
            .assert_value()
            .push(tokio::time::Instant::now());
        let body = self.bodies.lock().assert_value().pop_front().assert_value();
        RunHistoryResponse::new(StatusCode::OK.as_u16(), body)
    }
}

fn ui_get(client: &reqwest::Client, server: &LocalServer, path: &str) -> reqwest::RequestBuilder {
    client
        .get(format!("{}{}", server.origin, path))
        .header(header::ORIGIN, &server.origin)
}

fn fixed_remote(value: &impl Serialize) -> NativeRunHistory {
    NativeRunHistory::transported(Arc::new(FixedTransport {
        status: StatusCode::OK.as_u16(),
        body: Arc::new(serde_json::to_vec(value).assert_value()),
        routes: Arc::new(Mutex::new(Vec::new())),
    }))
}

#[tokio::test]
async fn local_ui_reads_a_real_target_history_without_moving_profiles_or_browser_authority() {
    let fixture = Fixture::new().await;
    populate_long_history(&fixture).await;
    let target = target_server(&fixture).await;
    let local = local_server(&target.origin).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .assert_value();
    assert_local_workspace(&client, &local, &target).await;
    let head = assert_remote_run_pages(&client, &local, &fixture).await;
    assert_live_append(&client, &local, &fixture, &head).await;
    assert_control_plane_isolation(&client, &local).await;
}

async fn populate_long_history(fixture: &Fixture) {
    fixture.start(1).await;
    let events = safe_logs(MAX_REPLAY_EVENTS + 12, "remote line");
    fixture
        .ledger
        .append(&fixture.id, events)
        .await
        .assert_value();
}

async fn assert_local_workspace(
    client: &reqwest::Client,
    local: &LocalServer,
    target: &TargetServer,
) {
    let local_bootstrap: Value = ui_get(client, local, "/ui/api/bootstrap")
        .send()
        .await
        .assert_value()
        .json()
        .await
        .assert_value();
    let target_bootstrap: Value = client
        .get(format!("{}/ui/api/bootstrap", target.origin))
        .send()
        .await
        .assert_value()
        .json()
        .await
        .assert_value();
    assert_eq!(local_bootstrap["workspace"]["kind"], "local");
    assert_eq!(target_bootstrap["workspace"]["kind"], "target");
    assert_ne!(
        local_bootstrap["workspace"]["id"],
        target_bootstrap["workspace"]["id"]
    );
}

async fn assert_remote_run_pages(
    client: &reqwest::Client,
    local: &LocalServer,
    fixture: &Fixture,
) -> String {
    let list: Value = ui_get(client, local, "/ui/api/runs")
        .send()
        .await
        .assert_value()
        .json()
        .await
        .assert_value();
    assert_eq!(list["runs"][0]["runId"], fixture.id.as_str());

    let detail: Value = ui_get(
        client,
        local,
        &format!("/ui/api/runs/{}", fixture.id.as_str()),
    )
    .send()
    .await
    .assert_value()
    .json()
    .await
    .assert_value();
    assert_eq!(detail["version"], 1);
    assert_eq!(detail["projectionVersion"], 1);
    assert_eq!(
        detail["initialInput"]["request"],
        "Write a migration report"
    );

    let first: Value = ui_get(
        client,
        local,
        &format!("/ui/api/runs/{}/history?after=v2%3A0", fixture.id.as_str()),
    )
    .send()
    .await
    .assert_value()
    .json()
    .await
    .assert_value();
    assert_eq!(
        first["events"].as_array().assert_value().len(),
        MAX_REPLAY_EVENTS
    );
    assert_eq!(first["complete"], false);
    let first_cursor = first["nextCursor"].as_str().assert_value();
    let second: Value = ui_get(
        client,
        local,
        &format!(
            "/ui/api/runs/{}/history?after={}",
            fixture.id.as_str(),
            first_cursor.replace(':', "%3A")
        ),
    )
    .send()
    .await
    .assert_value()
    .json()
    .await
    .assert_value();
    assert_eq!(second["events"][0]["cursor"], "v2:257");
    assert_eq!(second["events"].as_array().assert_value().len(), 14);
    assert_eq!(second["complete"], true);
    second["nextCursor"].as_str().assert_value().to_owned()
}

async fn assert_live_append(
    client: &reqwest::Client,
    local: &LocalServer,
    fixture: &Fixture,
    head: &str,
) {
    let mut response = ui_get(
        client,
        local,
        &format!(
            "/ui/api/runs/{}/events?after={}",
            fixture.id.as_str(),
            head.replace(':', "%3A")
        ),
    )
    .send()
    .await
    .assert_value();
    assert_eq!(response.status(), StatusCode::OK);
    let first_frame = response.chunk().await.assert_value().assert_value();
    assert!(String::from_utf8_lossy(&first_frame).contains("event: history"));

    fixture
        .ledger
        .append(
            &fixture.id,
            vec![RunEvent::SafeLog {
                execution: Some(ExecutionId::new(1).assert_value()),
                timestamp: UnixTimestampMillis::new(10_000).assert_value(),
                stream: SafeLogStream::Output,
                line: SafeLogLine::new("arrived through target polling").assert_value(),
            }],
        )
        .await
        .assert_value();
    let observed = tokio::time::timeout(Duration::from_secs(3), async {
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.assert_value() {
            body.extend_from_slice(&chunk);
            if String::from_utf8_lossy(&body).contains("arrived through target polling") {
                return true;
            }
        }
        false
    })
    .await
    .assert_value();
    assert!(observed);
}

async fn assert_control_plane_isolation(client: &reqwest::Client, local: &LocalServer) {
    // Selecting a history source cannot turn the local BFF into a target control-plane proxy.
    let response = ui_get(
        client,
        local,
        crate::native_v2_target_authority::DISCOVERY_PATH,
    )
    .send()
    .await
    .assert_value();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = client
        .post(format!(
            "{}{}",
            local.origin,
            crate::native_v2_target_authority::RUN_PATH
        ))
        .header(header::ORIGIN, &local.origin)
        .header(header::CONTENT_TYPE, "application/json")
        .body("{}")
        .send()
        .await
        .assert_value();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn remote_history_refuses_oversized_bodies_and_invalid_shared_definitions() {
    let oversized = FixedTransport {
        status: StatusCode::OK.as_u16(),
        body: Arc::new(vec![
            b' ';
            RunHistoryRequest::list(None).maximum_response_bytes()
                + 1
        ]),
        routes: Arc::new(Mutex::new(Vec::new())),
    };
    let source = NativeRunHistory::transported(Arc::new(oversized));
    assert_eq!(
        source.list(None).await.err().assert_value().code,
        "history_unavailable"
    );

    let fixture = Fixture::new().await;
    let definition = fixture.service.detail(&fixture.id).await.assert_value();
    let mut wrong_identity = definition.clone();
    wrong_identity["runId"] = json!(uuid::Uuid::now_v7().to_string());
    let mut wrong_version = definition.clone();
    wrong_version["projectionVersion"] = json!(2);
    let mut noncanonical_cursor = definition;
    noncanonical_cursor["cursor"] = json!("v2:00");
    for invalid in [wrong_identity, wrong_version, noncanonical_cursor] {
        let source = fixed_remote(&invalid);
        assert_eq!(
            source.detail(&fixture.id).await.err().assert_value().code,
            "history_incompatible"
        );
    }
}

#[tokio::test]
async fn remote_page_validation_rejects_gaps_and_false_completion() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    let value = fixture.service.page(&fixture.id, None).await.assert_value();
    let mut page: HistoryPage = serde_json::from_value(value).assert_value();
    page.events[1]["cursor"] = json!("v2:3");
    for invalid in [page.clone(), {
        page.events[1]["cursor"] = json!("v2:2");
        page.next_cursor = page.head_cursor.clone();
        page.complete = false;
        page
    }] {
        let source = fixed_remote(&invalid);
        assert_eq!(
            source
                .page(&fixture.id, Some(initial_cursor()))
                .await
                .err()
                .assert_value()
                .code,
            "history_incompatible"
        );
    }
}

fn scripted_page(after: u64, event: Option<u64>, terminal: bool) -> HistoryPage {
    let next = event.unwrap_or(after);
    serde_json::from_value(json!({
        "events": event.map(|cursor| vec![json!({"cursor":format!("v2:{cursor}")})])
            .unwrap_or_default(),
        "nextCursor": format!("v2:{next}"),
        "headCursor": format!("v2:{next}"),
        "complete": true,
        "finished": terminal,
        "observation": {"state": if terminal { "complete" } else { "collecting" }}
    }))
    .assert_value()
}

#[tokio::test(start_paused = true)]
async fn remote_live_polling_backs_off_caps_resets_and_remains_demand_driven() {
    let first = scripted_page(0, None, false);
    let idle_at_zero = first.clone();
    let material_at_one = scripted_page(0, Some(1), false);
    let idle_at_one = scripted_page(1, None, false);
    let terminal_at_two = scripted_page(1, Some(2), true);
    let pages = [
        first,
        idle_at_zero.clone(),
        idle_at_zero.clone(),
        idle_at_zero.clone(),
        idle_at_zero,
        material_at_one,
        idle_at_one,
        terminal_at_two,
    ];
    let calls = Arc::new(Mutex::new(Vec::new()));
    let transport = ScriptedTransport {
        bodies: Arc::new(Mutex::new(
            pages
                .into_iter()
                .map(|page| serde_json::to_vec(&page).assert_value())
                .collect(),
        )),
        calls: calls.clone(),
    };
    let source = NativeRunHistory::transported(Arc::new(transport));
    let id = RunId::new(uuid::Uuid::now_v7().to_string());
    let stream = source.subscribe(id, None).await.assert_value();
    futures_util::pin_mut!(stream);
    assert_eq!(
        stream
            .next()
            .await
            .assert_value()
            .assert_value()
            .next_cursor
            .as_str(),
        "v2:0"
    );

    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(calls.lock().assert_value().len(), 1);
    let following_started = tokio::time::Instant::now();
    assert_eq!(
        stream
            .next()
            .await
            .assert_value()
            .assert_value()
            .next_cursor
            .as_str(),
        "v2:1"
    );
    assert_eq!(
        stream
            .next()
            .await
            .assert_value()
            .assert_value()
            .next_cursor
            .as_str(),
        "v2:2"
    );
    assert!(stream.next().await.is_none());

    let observed = calls
        .lock()
        .assert_value()
        .iter()
        .skip(1)
        .map(|call| call.duration_since(following_started))
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        [500, 1_500, 3_500, 7_500, 11_500, 12_000, 13_000].map(Duration::from_millis)
    );
}

#[test]
fn remote_list_requires_descending_unique_ids_and_an_exact_page_cursor() {
    fn queued(id: &str) -> RunHistorySummary {
        RunHistorySummary {
            run_id: RunId::new(id),
            title: RunTitle::new("Queued run").assert_value(),
            phase: RunHistoryPhase::Queued,
            cursor: None,
            terminal: None,
            source: None,
            created_at: None,
            history_available: false,
            runtime_failure: None,
        }
    }

    let newer = queued("018f5e78-7f95-7c22-8d98-3f15af20c992");
    let older = queued("018f5e78-7f95-7c22-8d98-3f15af20c991");
    assert!(
        validate_remote_list(
            RunHistoryList {
                runs: vec![newer.clone(), older.clone()],
                next_cursor: Some(older.run_id.clone()),
            },
            None,
        )
        .is_ok()
    );
    assert!(
        validate_remote_list(
            RunHistoryList {
                runs: vec![newer.clone()],
                next_cursor: None,
            },
            Some(&newer.run_id),
        )
        .is_err()
    );
    assert!(
        validate_remote_list(
            RunHistoryList {
                runs: vec![older.clone()],
                next_cursor: None,
            },
            Some(&newer.run_id),
        )
        .is_ok()
    );
    for invalid in [
        RunHistoryList {
            runs: vec![older.clone(), newer.clone()],
            next_cursor: None,
        },
        RunHistoryList {
            runs: vec![newer.clone(), newer.clone()],
            next_cursor: None,
        },
        RunHistoryList {
            runs: vec![newer.clone(), older],
            next_cursor: Some(newer.run_id.clone()),
        },
        RunHistoryList {
            runs: Vec::new(),
            next_cursor: Some(newer.run_id),
        },
    ] {
        assert!(validate_remote_list(invalid, None).is_err());
    }
}

#[test]
fn remote_summary_validation_preserves_terminal_authority() {
    let id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c992");
    let title = RunTitle::new("Remote run").assert_value();
    let queued = RunHistorySummary {
        run_id: id.clone(),
        title: title.clone(),
        phase: RunHistoryPhase::Queued,
        cursor: None,
        terminal: None,
        source: None,
        created_at: None,
        history_available: false,
        runtime_failure: None,
    };
    assert!(!invalid_remote_summary(&queued));

    let failure = RuntimeFailure {
        at_cursor: Cursor::new("v2:7"),
        reason: "runtime_failed".into(),
    };
    let valid_failed = RunHistorySummary {
        phase: RunHistoryPhase::Finished,
        cursor: Some(failure.at_cursor.clone()),
        terminal: Some(RunHistoryTerminalSynopsis::Failed {
            reason: failure.reason.clone(),
        }),
        history_available: true,
        runtime_failure: Some(failure),
        ..queued.clone()
    };
    assert!(!invalid_remote_summary(&valid_failed));

    let mut invalid = Vec::new();
    invalid.push(RunHistorySummary {
        run_id: RunId::new("not-a-run-id"),
        ..queued.clone()
    });
    invalid.push(RunHistorySummary {
        cursor: Some(Cursor::new("v2:18446744073709551616")),
        history_available: true,
        ..queued.clone()
    });
    invalid.push(RunHistorySummary {
        cursor: Some(Cursor::new("v2:1")),
        ..queued.clone()
    });
    invalid.push(RunHistorySummary {
        phase: RunHistoryPhase::Unavailable,
        cursor: Some(Cursor::new("v2:1")),
        history_available: true,
        ..queued.clone()
    });
    invalid.push(RunHistorySummary {
        phase: RunHistoryPhase::Running,
        terminal: Some(RunHistoryTerminalSynopsis::Succeeded {}),
        ..queued.clone()
    });
    invalid.push(RunHistorySummary {
        terminal: None,
        ..valid_failed.clone()
    });
    invalid.push(RunHistorySummary {
        phase: RunHistoryPhase::Running,
        ..valid_failed.clone()
    });
    invalid.push(RunHistorySummary {
        terminal: Some(RunHistoryTerminalSynopsis::Failed {
            reason: "runtime_lost".into(),
        }),
        ..valid_failed
    });
    for summary in invalid {
        assert!(invalid_remote_summary(&summary));
    }
}

#[test]
fn remote_problem_and_transport_failures_remain_bounded_and_typed() {
    let problem = openengine_cluster_protocol::TargetHttpProblem::new(
        "history_pending",
        "History is still being prepared.",
        None,
    )
    .assert_value();
    let parsed = remote_problem(
        StatusCode::SERVICE_UNAVAILABLE,
        &serde_json::to_vec(&problem).assert_value(),
    );
    assert_eq!(parsed.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(parsed.code, "history_pending");
    assert_eq!(parsed.message, "History is still being prepared.");

    for body in [b"{".as_slice(), br#"{"code":"private","message":"hidden"}"#] {
        assert_eq!(
            remote_problem(StatusCode::BAD_GATEWAY, body).code,
            "history_unavailable"
        );
    }
    assert_eq!(
        transport_error(RunHistoryTransportError::Incompatible).code,
        "history_incompatible"
    );
    assert_eq!(
        transport_error(RunHistoryTransportError::Unavailable).code,
        "history_unavailable"
    );
}

#[tokio::test]
async fn hosted_pending_problem_reaches_history_and_initial_sse_read_unchanged() {
    let routes = Arc::new(Mutex::new(Vec::new()));
    let problem = openengine_cluster_protocol::TargetHttpProblem::new(
        "history_pending",
        "History is still being prepared.",
        None,
    )
    .assert_value();
    let transport = FixedTransport {
        status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        body: Arc::new(serde_json::to_vec(&problem).assert_value()),
        routes: routes.clone(),
    };
    let local = local_server_with_runs(NativeRunHistory::transported(Arc::new(transport))).await;
    let client = reqwest::Client::new();
    let id = uuid::Uuid::now_v7().to_string();
    for suffix in ["history?after=v2%3A0", "events?after=v2%3A0"] {
        let response = ui_get(&client, &local, &format!("/ui/api/runs/{id}/{suffix}"))
            .send()
            .await
            .assert_value();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.json::<Value>().await.assert_value(),
            json!({
                "code": "history_pending",
                "message": "History is still being prepared."
            })
        );
    }
    assert_eq!(
        *routes.lock().assert_value(),
        [
            format!("page:{id}?after=v2:0"),
            format!("page:{id}?after=v2:0"),
        ]
    );
}
