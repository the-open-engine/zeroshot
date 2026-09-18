use super::*;
use super::super::status::confirmed_failure;
use openengine_cluster_protocol::{RunStatus, RunStatusParams, RunStatusResult};

async fn failed_runtime() -> (Fixture, NativeRunHistory, NativeV2Observability) {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    let snapshot = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value()
        .snapshot;
    let observations = fixture.observations();
    observations.track_runtime(&snapshot).assert_value();
    observations.runtime_failed(&fixture.id);
    let source = NativeRunHistory::target(
        fixture.root.join("runs").join(fixture.id.as_str()),
        observations.clone(),
    );
    (fixture, source, observations)
}

#[tokio::test]
async fn confirmed_failure_reports_current_state_without_fabricating_history() {
    let (fixture, source, _) = failed_runtime().await;
    let before = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value();
    let detail = source.detail(&fixture.id).await.assert_value();
    assert_eq!(detail["phase"], "finished");
    assert_eq!(
        detail["terminal"],
        json!({"status":"failed", "reason":"runtime_failed"})
    );
    assert_eq!(
        detail["runtimeFailure"],
        json!({"atCursor":"v2:2", "reason":"runtime_failed"})
    );
    assert_eq!(detail["snapshot"]["phase"], "running");
    assert!(detail["snapshot"]["terminal"].is_null());
    assert_eq!(detail["history"]["complete"], false);
    let listed = source.list(None).await.assert_value();
    assert_eq!(
        listed["runs"][0]["runtimeFailure"],
        detail["runtimeFailure"]
    );
    assert_eq!(listed["runs"][0]["phase"], "finished");

    let stream = source
        .subscribe(fixture.id.clone(), None)
        .await
        .assert_value();
    futures_util::pin_mut!(stream);
    let page = stream.next().await.assert_value().assert_value();
    assert!(page.complete && page.finished);
    assert_eq!(page.next_cursor.as_str(), "v2:2");
    assert_eq!(json!(page.runtime_failure), detail["runtimeFailure"]);
    assert_eq!(page.events.len(), 2);
    assert!(
        page.events
            .iter()
            .all(|event| event["event"]["kind"] != "terminal")
    );
    assert!(stream.next().await.is_none());
    let after = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value();
    assert_eq!(before, after);
}

#[tokio::test]
async fn durable_terminal_wins_over_stale_runtime_failure() {
    let (fixture, source, observations) = failed_runtime().await;
    fixture
        .ledger
        .append(
            &fixture.id,
            vec![live::completed(&fixture, 1), live::terminal()],
        )
        .await
        .assert_value();
    // The source still has its old fallback; the UI must prefer the durable terminal regardless.
    assert!(matches!(
        observations
            .status(RunStatusParams {
                run_id: fixture.id.clone()
            })
            .await
            .assert_value()
            .status,
        RunStatus::Finished {
            terminal_result: TerminalResult::Failed { .. },
            ..
        }
    ));
    let detail = source.detail(&fixture.id).await.assert_value();
    assert_eq!(detail["terminal"]["status"], "succeeded");
    assert_eq!(detail["history"]["complete"], true);
    assert!(detail.get("runtimeFailure").is_none());
    let page = source.page(&fixture.id, None).await.assert_value();
    assert!(page.get("runtimeFailure").is_none());
    assert_eq!(page["finished"], true);
    let list = source.list(None).await.assert_value();
    assert_eq!(list["runs"][0]["terminal"]["status"], "succeeded");
    assert!(list["runs"][0].get("runtimeFailure").is_none());
}

#[tokio::test]
async fn failed_runtime_drains_all_retained_pages_and_reconnects_at_the_same_cursor() {
    let (fixture, source, _) = failed_runtime().await;
    fixture
        .ledger
        .append(
            &fixture.id,
            (0..MAX_REPLAY_EVENTS + 3)
                .map(|index| RunEvent::SafeLog {
                    execution: Some(ExecutionId::new(1).assert_value()),
                    timestamp: UnixTimestampMillis::new(index as u64 + 1).assert_value(),
                    stream: SafeLogStream::Output,
                    line: SafeLogLine::new(format!("retained {index}")).assert_value(),
                })
                .collect(),
        )
        .await
        .assert_value();
    let stream = source
        .subscribe(fixture.id.clone(), None)
        .await
        .assert_value();
    futures_util::pin_mut!(stream);
    let first = stream.next().await.assert_value().assert_value();
    assert!(first.finished && !first.complete);
    assert_eq!(first.events.len(), MAX_REPLAY_EVENTS);
    assert!(first.runtime_failure.is_some());
    let second = stream.next().await.assert_value().assert_value();
    assert!(second.complete && second.finished);
    assert_eq!(second.events.len(), 5);
    assert!(stream.next().await.is_none());
    let resumed = source
        .page(&fixture.id, Some(first.next_cursor))
        .await
        .assert_value();
    assert_eq!(resumed, json!(second));
}

#[tokio::test]
async fn caught_up_stream_reports_later_runtime_failure_without_an_extra_event() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    let observations = fixture.observations();
    let snapshot = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value()
        .snapshot;
    observations.track_runtime(&snapshot).assert_value();
    let source = NativeRunHistory::target(
        fixture.root.join("runs").join(fixture.id.as_str()),
        observations.clone(),
    );
    let stream = source
        .subscribe(fixture.id.clone(), None)
        .await
        .assert_value();
    futures_util::pin_mut!(stream);
    let first = stream.next().await.assert_value().assert_value();
    assert!(first.complete && !first.finished);
    observations.runtime_failed(&fixture.id);
    let failure = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .assert_value()
        .assert_value()
        .assert_value();
    assert!(failure.events.is_empty());
    assert_eq!(failure.next_cursor, first.next_cursor);
    assert!(failure.complete && failure.finished && failure.runtime_failure.is_some());
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn missing_local_or_target_status_cannot_finish_or_repair_retained_history() {
    let mut fixture = Fixture::new().await;
    fixture.service.status = Some(RuntimeStatusReader::Local(fixture.root.clone()));
    fixture.start(1).await;
    fixture
        .ledger
        .append(
            &fixture.id,
            (0..MAX_REPLAY_EVENTS)
                .map(|index| RunEvent::SafeLog {
                    execution: Some(ExecutionId::new(1).assert_value()),
                    timestamp: UnixTimestampMillis::new(index as u64 + 1).assert_value(),
                    stream: SafeLogStream::Output,
                    line: SafeLogLine::new(format!("retained {index}")).assert_value(),
                })
                .collect(),
        )
        .await
        .assert_value();
    let target = NativeRunHistory::target(
        fixture.root.join("runs").join(fixture.id.as_str()),
        NativeV2Observability::new(Arc::new(crate::v2_run_ledger::fake::FakeRunLedger::new())),
    );
    let before = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value();
    for source in [&fixture.service, &target] {
        let detail = source.detail(&fixture.id).await.assert_value();
        assert_eq!(detail["phase"], "running");
        assert!(detail["terminal"].is_null());
        assert!(detail.get("runtimeFailure").is_none());
        assert_eq!(
            source.page(&fixture.id, None).await.assert_value()["finished"],
            false
        );
        let stream = source
            .subscribe(fixture.id.clone(), None)
            .await
            .assert_value();
        futures_util::pin_mut!(stream);
        let first = stream.next().await.assert_value().assert_value();
        assert_eq!(first.events.len(), MAX_REPLAY_EVENTS);
        assert!(!first.complete && !first.finished);
        let last = stream.next().await.assert_value().assert_value();
        assert_eq!(last.events.len(), 2);
        assert!(last.complete && !last.finished);
        let error = stream.next().await.assert_value().err().assert_value();
        assert_eq!(error.code, "runtime_unavailable");
        assert!(stream.next().await.is_none());
    }
    assert_eq!(
        before,
        fixture
            .ledger
            .get(&fixture.id)
            .await
            .assert_value()
            .assert_value()
    );
    let directory = fixture.root.join("runs").join(fixture.id.as_str());
    for name in [
        "controller.lock",
        "controller.sock",
        "controller.ready.json",
        "runtime",
    ] {
        assert!(!directory.join(name).exists());
    }
}

#[tokio::test]
async fn failure_observation_requires_exact_run_canonical_cursor_and_reserved_reason() {
    let (fixture, _, observations) = failed_runtime().await;
    let snapshot = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value()
        .snapshot;
    let valid = observations
        .status(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .assert_value();
    assert!(confirmed_failure(&snapshot, valid.clone()).is_some());
    let mut wrong_run = valid.clone();
    wrong_run.run_id = RunId::new(uuid::Uuid::now_v7().to_string());
    assert!(confirmed_failure(&snapshot, wrong_run).is_none());
    for cursor in ["bad", "v2:02", "v2:+2", "v2:3", "v2:18446744073709551615"] {
        let mut wrong_cursor = valid.clone();
        wrong_cursor.at_cursor = Cursor::new(cursor);
        assert!(confirmed_failure(&snapshot, wrong_cursor).is_none());
    }
    let mut other_result = valid;
    other_result.status = RunStatus::Finished {
        terminal_result: TerminalResult::Succeeded {
            output: Value::Null,
        },
        metadata: Default::default(),
    };
    assert!(confirmed_failure(&snapshot, other_result).is_none());
}

#[cfg(unix)]
mod local {
    use super::*;
    use crate::native_v2_portable_controller::{PortableControllerPaths, PortableControllerReady};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    struct SocketDirectory(PathBuf);
    impl SocketDirectory {
        fn new() -> Self {
            // Keep the full path below macOS's 104-byte Unix socket limit.
            Self(PathBuf::from(format!(
                "/tmp/zsu-{}",
                uuid::Uuid::now_v7().simple()
            )))
        }
        fn paths(&self, id: &RunId) -> PortableControllerPaths {
            PortableControllerPaths::new(self.0.join("runs").join(id.as_str()))
        }
        fn listen(&self, id: &RunId) -> UnixListener {
            let paths = self.paths(id);
            std::fs::create_dir_all(paths.storage()).assert_value();
            let listener = UnixListener::bind(paths.socket()).assert_value();
            self.ready(id, id);
            listener
        }
        fn ready(&self, path_id: &RunId, ready_id: &RunId) {
            let paths = self.paths(path_id);
            std::fs::write(
                paths.ready(),
                serde_json::to_vec(&PortableControllerReady {
                    kind: "zeroshot.portable-controller-ready/v1".into(),
                    run_id: ready_id.clone(),
                    socket: paths.socket(),
                    pid: std::process::id(),
                })
                .assert_value(),
            )
            .assert_value();
        }
        fn assert_no_observer(&self, id: &RunId) {
            let paths = self.paths(id);
            assert!(!paths.lease().exists());
            assert!(!paths.ledger().exists());
            assert!(!paths.runtime().exists());
        }
    }
    impl Drop for SocketDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn reply(listener: UnixListener, result: RunStatusResult) {
        let (stream, _) = listener.accept().await.assert_value();
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        let mut line = String::new();
        reader.read_line(&mut line).await.assert_value();
        let request: Value = serde_json::from_str(&line).assert_value();
        assert_eq!(
            request["method"],
            openengine_cluster_protocol::RUN_STATUS_METHOD
        );
        assert_eq!(request["params"]["runId"], result.run_id.as_str());
        let response = json!({"jsonrpc":"2.0", "id":request["id"], "result":result});
        write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .assert_value();
        let mut remaining = Vec::new();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), reader.read_to_end(&mut remaining))
                .await
                .assert_value()
                .assert_value(),
            0
        );
    }

    #[tokio::test]
    async fn local_status_uses_existing_socket_then_closes_without_creating_an_observer() {
        let (fixture, _, observations) = failed_runtime().await;
        let snapshot = fixture
            .ledger
            .get(&fixture.id)
            .await
            .assert_value()
            .assert_value()
            .snapshot;
        let status = observations
            .status(RunStatusParams {
                run_id: fixture.id.clone(),
            })
            .await
            .assert_value();
        let directory = SocketDirectory::new();
        let server = tokio::spawn(reply(directory.listen(&fixture.id), status));
        let source = RuntimeStatusReader::Local(directory.0.clone());
        assert!(source.failure(&snapshot).await.assert_value().is_some());
        server.await.assert_value();
        directory.assert_no_observer(&fixture.id);
        assert_eq!(
            snapshot,
            fixture
                .ledger
                .get(&fixture.id)
                .await
                .assert_value()
                .assert_value()
                .snapshot
        );
    }

    #[tokio::test]
    async fn wrong_readiness_identity_is_rejected_before_connecting() {
        let (fixture, _, _) = failed_runtime().await;
        let snapshot = fixture
            .ledger
            .get(&fixture.id)
            .await
            .assert_value()
            .assert_value()
            .snapshot;
        let directory = SocketDirectory::new();
        let listener = directory.listen(&fixture.id);
        directory.ready(&fixture.id, &RunId::new(uuid::Uuid::now_v7().to_string()));
        let source = RuntimeStatusReader::Local(directory.0.clone());
        assert!(source.failure(&snapshot).await.is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
        directory.assert_no_observer(&fixture.id);
    }

    #[tokio::test]
    async fn cancelling_or_timing_out_a_status_read_closes_its_socket() {
        let (fixture, _, _) = failed_runtime().await;
        let snapshot = fixture
            .ledger
            .get(&fixture.id)
            .await
            .assert_value()
            .assert_value()
            .snapshot;
        for cancel in [true, false] {
            let directory = SocketDirectory::new();
            let listener = directory.listen(&fixture.id);
            let (seen, request) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.assert_value();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).await.assert_value();
                seen.send(()).assert_value();
                let mut rest = Vec::new();
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(2), reader.read_to_end(&mut rest))
                        .await
                        .assert_value()
                        .assert_value(),
                    0
                );
            });
            let source = RuntimeStatusReader::Local(directory.0.clone());
            let snapshot = snapshot.clone();
            let operation = tokio::spawn(async move { source.failure(&snapshot).await });
            request.await.assert_value();
            if cancel {
                operation.abort();
                assert!(operation.await.is_err_and(|error| error.is_cancelled()));
            } else {
                assert!(operation.await.assert_value().is_err());
            }
            server.await.assert_value();
            directory.assert_no_observer(&fixture.id);
        }
    }
}
