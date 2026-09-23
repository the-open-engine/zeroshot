use super::*;
use axum::http::HeaderValue;

#[tokio::test]
async fn ui_shutdown_closes_a_live_sse_without_stopping_the_run() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .assert_value();
    let address = listener.local_addr().assert_value();
    let service = crate::profile_ui::UiService::for_target(
        fixture.root.join("runs").join(fixture.id.as_str()),
        &format!("http://{address}"),
        fixture.observations(),
    )
    .assert_value();
    let serving = service.clone();
    let task = tokio::spawn(async move {
        let (connection, _) = listener.accept().await.assert_value();
        drop(listener);
        serving.serve_connection(connection).await.assert_value();
    });
    let mut response = reqwest::Client::new()
        .get(format!(
            "http://{address}/ui/api/runs/{}/events",
            fixture.id.as_str()
        ))
        .send()
        .await
        .assert_value();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.chunk().await.assert_value().is_some());
    service.shutdown();
    tokio::time::timeout(Duration::from_secs(2), response.text())
        .await
        .assert_value()
        .assert_value();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .assert_value()
        .assert_value();
    let stored = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value();
    assert!(stored.snapshot.terminal.is_none());
    assert!(!stored.snapshot.force_stop_requested);
    assert_eq!(stored.snapshot.cursor.as_str(), "v2:2");
}

pub(super) fn terminal() -> RunEvent {
    RunEvent::Terminal {
        result: TerminalResult::Succeeded {
            output: Value::Null,
        },
    }
}

pub(super) fn completed(fixture: &Fixture, execution: u64) -> RunEvent {
    RunEvent::NodeCompleted {
        completion: NodeCompletion {
            reference: fixture.reference(execution),
            outcome: WorkerOutcome::Verified {
                output: Value::Null,
                artifacts: vec![],
            },
        },
    }
}

#[tokio::test]
async fn live_history_starts_empty_then_observes_appended_execution_and_terminal() {
    let fixture = Fixture::new().await;
    let stream = fixture
        .service
        .subscribe(fixture.id.clone(), None)
        .await
        .assert_value();
    futures_util::pin_mut!(stream);
    let first = stream.next().await.assert_value().assert_value();
    assert!(first.events.is_empty());
    assert_eq!(first.next_cursor.as_str(), "v2:0");
    assert!(first.complete);
    assert!(!first.finished);

    // A caught-up connection stays open without repeatedly sending empty pages.
    assert!(
        tokio::time::timeout(Duration::from_millis(25), stream.next())
            .await
            .is_err()
    );
    fixture.start(9_007_199_254_740_993).await;
    let page = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .assert_value()
        .assert_value()
        .assert_value();
    assert_eq!(page.next_cursor.as_str(), "v2:2");
    assert_eq!(
        page.events[1]["event"]["reference"]["execution"],
        "9007199254740993"
    );
    assert!(!page.finished);

    fixture
        .ledger
        .append(
            &fixture.id,
            vec![completed(&fixture, 9_007_199_254_740_993), terminal()],
        )
        .await
        .assert_value();
    let page = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .assert_value()
        .assert_value()
        .assert_value();
    assert_eq!(page.events.len(), 2);
    assert_eq!(page.events[0]["cursor"], "v2:3");
    assert!(page.complete && page.finished);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn terminal_stream_drains_bounded_pages_and_reconnect_resumes_exactly() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    let mut events = safe_logs(MAX_REPLAY_EVENTS + 12, "retained line");
    events.extend([completed(&fixture, 1), terminal()]);
    fixture
        .ledger
        .append(&fixture.id, events)
        .await
        .assert_value();
    let mut stream = Box::pin(
        fixture
            .service
            .subscribe(fixture.id.clone(), None)
            .await
            .assert_value(),
    );
    let first = stream.next().await.assert_value().assert_value();
    assert_eq!(first.events.len(), MAX_REPLAY_EVENTS);
    assert!(!first.complete && first.finished);
    drop(stream);

    let stream = fixture
        .service
        .subscribe(fixture.id.clone(), Some(first.next_cursor))
        .await
        .assert_value();
    futures_util::pin_mut!(stream);
    let resumed = stream.next().await.assert_value().assert_value();
    assert_eq!(resumed.events[0]["cursor"], "v2:257");
    assert_eq!(resumed.events.len(), 16);
    assert_eq!(resumed.next_cursor, resumed.head_cursor);
    assert!(resumed.complete && resumed.finished);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn stream_returns_cursor_errors_before_open_and_gap_errors_then_closes() {
    let fixture = Fixture::new().await;
    for cursor in ["bad", "v2:9999"] {
        let error = fixture
            .service
            .subscribe(fixture.id.clone(), Some(Cursor::new(cursor)))
            .await
            .err()
            .assert_value();
        assert_eq!(error.code, "invalid_cursor");
    }
    let stream = fixture
        .service
        .subscribe(fixture.id.clone(), None)
        .await
        .assert_value();
    futures_util::pin_mut!(stream);
    assert!(stream.next().await.assert_value().is_ok());
    fixture.start(1).await;
    let connection = rusqlite::Connection::open(
        fixture
            .root
            .join("runs")
            .join(fixture.id.as_str())
            .join("runs.sqlite3"),
    )
    .assert_value();
    connection
        .execute("DELETE FROM v2_run_events WHERE sequence=1", [])
        .assert_value();
    let error = stream.next().await.assert_value().err().assert_value();
    assert_eq!(error.code, "history_gap");
    assert!(stream.next().await.is_none());
}

#[test]
fn reconnect_header_takes_precedence_and_invalid_or_duplicate_headers_fail() {
    let query = || PageQuery {
        after: Some("v2:1".into()),
    };
    let mut headers = HeaderMap::new();
    headers.insert("last-event-id", HeaderValue::from_static("v2:7"));
    assert_eq!(
        resume_cursor(query(), &headers)
            .assert_value()
            .assert_value()
            .as_str(),
        "v2:7"
    );
    for value in ["", "bad", "v2:18446744073709551616"] {
        headers.insert("last-event-id", HeaderValue::from_str(value).assert_value());
        assert_eq!(
            resume_cursor(query(), &headers).err().assert_value().code,
            "invalid_cursor"
        );
    }
    headers.insert("last-event-id", HeaderValue::from_static("v2:1"));
    headers.append("last-event-id", HeaderValue::from_static("v2:2"));
    assert!(resume_cursor(query(), &headers).is_err());
}

#[tokio::test]
async fn sse_route_emits_named_pages_and_validates_resume_before_headers() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    fixture
        .ledger
        .append(&fixture.id, vec![completed(&fixture, 1), terminal()])
        .await
        .assert_value();
    let (authority, task) = serve_local_ui(&fixture).await;
    let client = reqwest::Client::new();
    let url = format!(
        "http://{authority}/ui/api/runs/{}/events?after=v2%3A0",
        fixture.id.as_str()
    );
    let response = client
        .get(&url)
        .header("last-event-id", "v2:3")
        .send()
        .await
        .assert_value();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = response.text().await.assert_value();
    assert!(body.contains("event: history\n"));
    assert!(body.contains("id: v2:4\n"));
    let page: Value = serde_json::from_str(
        body.lines()
            .find_map(|line| line.strip_prefix("data: "))
            .assert_value(),
    )
    .assert_value();
    assert_eq!(page["events"].as_array().assert_value().len(), 1);
    assert_eq!(page["events"][0]["cursor"], "v2:4");
    assert_eq!(page["finished"], true);
    for cursor in ["bad", "v2:999"] {
        let response = client
            .get(&url)
            .header("last-event-id", cursor)
            .send()
            .await
            .assert_value();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json::<Value>().await.assert_value()["code"],
            "invalid_cursor"
        );
    }
    task.abort();
}
