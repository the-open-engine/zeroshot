use super::*;
use std::sync::Arc;
use crate::full_v1_reducer::StructuralOccurrence;
use crate::native_v2_contract::{
    AdmittedRun, ExecutionId, ExecutionRef, NodeCompletion, NodeInstanceId,
};
use crate::native_v2_observability::NativeV2Observability;
use crate::v2_run_ledger::{CreateRun, RunEvent, SafeLogLine, SafeLogStream, MAX_REPLAY_EVENTS};
use openengine_cluster_protocol::{
    CompiledGraphIr, IdempotencyKey, NodeName, PositiveInteger, RunSize, RunTitle, Sha256Digest,
    SourceBranchId, SourceRepositoryId, SourceRevisionId, ResolvedSource, TerminalResult,
    UnixTimestampMillis, WorkerOutcome,
};
use openengine_cluster_testkit::assertions::AssertValue;

mod control;
mod live;
mod status;

#[tokio::test]
async fn target_shared_ledger_uses_the_same_definition_and_replay_projection() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    let storage = fixture.root.join("runs").join(fixture.id.as_str());
    let target = NativeRunHistory::target(storage.clone(), fixture.observations());
    assert_eq!(
        target.detail(&fixture.id).await.assert_value(),
        fixture.service.detail(&fixture.id).await.assert_value()
    );
    assert_eq!(
        target.page(&fixture.id, None).await.assert_value(),
        fixture.service.page(&fixture.id, None).await.assert_value()
    );
    let other = RunId::new(uuid::Uuid::now_v7().to_string());
    let admitted = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value()
        .admitted;
    fixture
        .ledger
        .create_or_get(CreateRun {
            run_id: other.clone(),
            submission_key: IdempotencyKey::new("another-target-run").assert_value(),
            submission_digest: Sha256Digest::new("b".repeat(64)).assert_value(),
            admitted,
        })
        .await
        .assert_value();
    let listed = NativeRunHistory::target(storage, fixture.observations())
        .list(None)
        .await
        .assert_value();
    assert_eq!(listed["runs"].as_array().assert_value().len(), 2);
    assert_eq!(listed["runs"][0]["runId"], other.as_str());
    assert_eq!(
        fixture.service.list(None).await.assert_value()["runs"]
            .as_array()
            .assert_value()
            .len(),
        1
    );
}

#[tokio::test]
async fn empty_target_history_does_not_create_storage_or_a_controller() {
    let root = std::env::temp_dir().join(format!("zeroshot-empty-ui-{}", uuid::Uuid::now_v7()));
    let target = NativeRunHistory::target(
        root.clone(),
        NativeV2Observability::new(Arc::new(crate::v2_run_ledger::fake::FakeRunLedger::new())),
    );
    assert_eq!(target.list(None).await.assert_value()["runs"], json!([]));
    assert!(!root.exists());
}

#[tokio::test]
async fn target_list_pages_are_stable_and_isolate_corrupt_run_payloads() {
    let fixture = Fixture::new().await;
    let storage = fixture.root.join("runs").join(fixture.id.as_str());
    let admitted = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value()
        .admitted;
    let mut ids = vec![fixture.id.clone()];
    for index in 0..LIST_PAGE_SIZE + 2 {
        let id = RunId::new(uuid::Uuid::now_v7().to_string());
        fixture
            .ledger
            .create_or_get(CreateRun {
                run_id: id.clone(),
                submission_key: IdempotencyKey::new(format!("paged-target-{index}")).assert_value(),
                submission_digest: Sha256Digest::new("b".repeat(64)).assert_value(),
                admitted: admitted.clone(),
            })
            .await
            .assert_value();
        ids.push(id);
    }
    // Corruption outside the requested page must not require decoding that run or hide all runs.
    let connection = rusqlite::Connection::open(storage.join("runs.sqlite3")).assert_value();
    connection
        .execute(
            "UPDATE v2_runs SET stored_json = 'invalid retained run' WHERE run_id = ?1",
            rusqlite::params![fixture.id.as_str()],
        )
        .assert_value();
    let target = NativeRunHistory::target(storage, fixture.observations());
    let first = target.list(None).await.assert_value();
    assert_eq!(
        first["runs"].as_array().assert_value().len(),
        LIST_PAGE_SIZE
    );
    assert!(
        first["runs"]
            .as_array()
            .assert_value()
            .iter()
            .all(|run| run["historyAvailable"] == true)
    );
    let after = parse_id(first["nextCursor"].as_str().assert_value().to_owned())
        .ok()
        .assert_value();
    let fresh = RunId::new(uuid::Uuid::now_v7().to_string());
    fixture
        .ledger
        .create_or_get(CreateRun {
            run_id: fresh.clone(),
            submission_key: IdempotencyKey::new("newer-target-run").assert_value(),
            submission_digest: Sha256Digest::new("c".repeat(64)).assert_value(),
            admitted,
        })
        .await
        .assert_value();
    let second = target.list(Some(after)).await.assert_value();
    assert_eq!(second["runs"].as_array().assert_value().len(), 3);
    assert!(second["nextCursor"].is_null());
    assert_eq!(second["runs"][2]["runId"], fixture.id.as_str());
    assert_eq!(second["runs"][2]["historyAvailable"], false);
    let combined = first["runs"]
        .as_array()
        .assert_value()
        .iter()
        .chain(second["runs"].as_array().assert_value())
        .map(|run| run["runId"].as_str().assert_value())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(combined.len(), ids.len());
    assert!(!combined.contains(fresh.as_str()));
}

struct Fixture {
    root: PathBuf,
    service: NativeRunHistory,
    id: RunId,
    ledger: SqliteRunLedger,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    fn observations(&self) -> NativeV2Observability {
        NativeV2Observability::new(Arc::new(self.ledger.clone()))
    }

    async fn new() -> Self {
        let graph: CompiledGraphIr = serde_json::from_str(include_str!(
            "../../../../protocol/openengine-cluster/v1/fixtures/graph/canonical/base.json"
        ))
        .assert_value();
        Self::with_graph(graph, json!({"request":"Write a migration report"})).await
    }

    async fn new_with_controller_lease()
    -> (Self, crate::native_v2_portable_controller::ControllerLease) {
        let graph: CompiledGraphIr = serde_json::from_str(include_str!(
            "../../../../protocol/openengine-cluster/v1/fixtures/graph/canonical/base.json"
        ))
        .assert_value();
        let (fixture, lease) =
            Self::build(graph, json!({"request":"Write a migration report"}), true).await;
        (fixture, lease.assert_value())
    }

    async fn with_graph(graph: CompiledGraphIr, initial_input: Value) -> Self {
        Self::build(graph, initial_input, false).await.0
    }

    async fn build(
        graph: CompiledGraphIr,
        initial_input: Value,
        hold_controller_lease: bool,
    ) -> (
        Self,
        Option<crate::native_v2_portable_controller::ControllerLease>,
    ) {
        let root = std::env::temp_dir().join(format!("zeroshot-history-{}", uuid::Uuid::now_v7()));
        let id = RunId::new(uuid::Uuid::now_v7().to_string());
        let runs = root.join("runs");
        crate::execution::platform::private_directory(&runs)
            .assert_value_with("prepare profile UI fixture runs directory");
        let directory = runs.join(id.as_str());
        crate::execution::platform::create_private_directory(&directory)
            .assert_value_with("create profile UI fixture run directory");
        let paths =
            crate::native_v2_portable_controller::PortableControllerPaths::new(directory.clone());
        let controller_lease = hold_controller_lease.then(|| {
            crate::native_v2_portable_controller::ControllerLease::acquire(paths.lease())
                .assert_value_with("acquire embedded controller lease fixture")
        });
        let ledger = SqliteRunLedger::open(directory.join("runs.sqlite3"))
            .assert_value_with("open profile UI fixture ledger");
        ledger
            .create_or_get(CreateRun {
                run_id: id.clone(),
                submission_key: IdempotencyKey::new("test-history").assert_value(),
                submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
                admitted: AdmittedRun {
                    title: RunTitle::new("Inspect retained changes").assert_value(),
                    graph,
                    initial_input,
                    runtime: crate::native_v2_contract::RuntimePlan::Codex {
                        provider: crate::native_v2_contract::CodexProvider::OpenAi,
                        size: RunSize::Small,
                        nodes: Default::default(),
                    },
                    source: ResolvedSource {
                        repository: SourceRepositoryId::new("open-engine/test-matrix")
                            .assert_value(),
                        branch: SourceBranchId::new("main").assert_value(),
                        revision: SourceRevisionId::new("a".repeat(40)).assert_value(),
                    },
                },
            })
            .await
            .assert_value_with("create profile UI fixture run");
        (
            Self {
                service: NativeRunHistory::new(root.clone()),
                root,
                id,
                ledger,
            },
            controller_lease,
        )
    }
    fn reference(&self, execution: u64) -> ExecutionRef {
        ExecutionRef {
            run_id: self.id.clone(),
            node: NodeName::new("worker").assert_value(),
            node_instance: NodeInstanceId::new(execution).assert_value(),
            execution: ExecutionId::new(execution).assert_value(),
        }
    }
    async fn start(&self, execution: u64) {
        self.ledger
            .append(
                &self.id,
                vec![
                    RunEvent::RunStarted,
                    RunEvent::NodeStarted {
                        reference: self.reference(execution),
                        occurrence: StructuralOccurrence {
                            node: NodeName::new("worker").assert_value(),
                            map_indices: vec![2],
                        },
                        attempt: PositiveInteger::new(2).assert_value(),
                        input: json!({"request":"observed input"}),
                    },
                ],
            )
            .await
            .assert_value_with("append profile UI fixture start");
    }
}

#[tokio::test]
async fn snapshot_uses_admitted_definition_and_exact_browser_identities() {
    let fixture = Fixture::new().await;
    let identity = 9_007_199_254_740_993;
    fixture.start(identity).await;
    fixture
        .ledger
        .append(
            &fixture.id,
            vec![
                RunEvent::SafeLog {
                    execution: Some(ExecutionId::new(identity).assert_value()),
                    timestamp: UnixTimestampMillis::new(1_789_612_345_678).assert_value(),
                    stream: SafeLogStream::Output,
                    line: SafeLogLine::new("Report written.\nNext: review.").assert_value(),
                },
                RunEvent::NodeCompleted {
                    completion: NodeCompletion {
                        reference: fixture.reference(identity),
                        outcome: WorkerOutcome::Verified {
                            output: Value::Null,
                            artifacts: vec![],
                        },
                    },
                },
                RunEvent::Terminal {
                    result: TerminalResult::Succeeded {
                        output: json!({"report":"report.md"}),
                    },
                },
            ],
        )
        .await
        .assert_value();
    let detail = fixture.service.detail(&fixture.id).await.assert_value();
    assert_eq!(
        detail["initialInput"]["request"],
        "Write a migration report"
    );
    assert!(detail["graph"].get("bounds").is_none());
    assert!(detail["graph"]["root"].is_object());
    assert_eq!(
        detail["snapshot"]["executions"][identity.to_string()]["reference"]["execution"],
        identity.to_string()
    );
    let page = fixture.service.page(&fixture.id, None).await.assert_value();
    assert_eq!(
        page["events"][1]["event"]["reference"]["nodeInstance"],
        identity.to_string()
    );
    assert_eq!(
        page["events"][1]["event"]["occurrence"]["mapIndices"],
        json!([2])
    );
    assert_eq!(
        page["events"][2]["event"]["execution"],
        identity.to_string()
    );
    assert_eq!(
        page["events"][2]["event"]["timestamp"],
        1_789_612_345_678u64
    );
    assert_eq!(
        page["events"][3]["event"]["completion"]["reference"]["execution"],
        identity.to_string()
    );
    assert_eq!(page["complete"], true);
    assert_eq!(page["finished"], true);
    assert_eq!(page["nextCursor"], "v2:5");
    let listed = fixture.service.list(None).await.assert_value();
    assert_eq!(listed["runs"][0]["terminal"]["status"], "succeeded");
    assert!(listed["runs"][0]["createdAt"].as_u64().assert_value() > 0);
}

#[tokio::test]
async fn pages_remain_bounded_and_terminal_does_not_skip_retained_output() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    let mut events = (0..(MAX_REPLAY_EVENTS + 12))
        .map(|index| RunEvent::SafeLog {
            execution: Some(ExecutionId::new(1).assert_value()),
            timestamp: UnixTimestampMillis::new(index as u64 + 1).assert_value(),
            stream: SafeLogStream::Output,
            line: SafeLogLine::new(format!("line {index}")).assert_value(),
        })
        .collect::<Vec<_>>();
    events.push(RunEvent::NodeCompleted {
        completion: NodeCompletion {
            reference: fixture.reference(1),
            outcome: WorkerOutcome::Verified {
                output: Value::Null,
                artifacts: vec![],
            },
        },
    });
    events.push(RunEvent::Terminal {
        result: TerminalResult::Succeeded {
            output: Value::Null,
        },
    });
    fixture
        .ledger
        .append(&fixture.id, events)
        .await
        .assert_value();
    let first = fixture.service.page(&fixture.id, None).await.assert_value();
    assert_eq!(
        first["events"].as_array().assert_value().len(),
        MAX_REPLAY_EVENTS
    );
    assert_eq!(first["complete"], false);
    assert_eq!(first["finished"], true);
    let second = fixture
        .service
        .page(
            &fixture.id,
            Some(Cursor::new(first["nextCursor"].as_str().assert_value())),
        )
        .await
        .assert_value();
    assert_eq!(second["events"][0]["cursor"], "v2:257");
    assert_eq!(second["complete"], true);
    assert_eq!(second["nextCursor"], second["headCursor"]);
    let at_head = fixture
        .service
        .page(
            &fixture.id,
            Some(Cursor::new(second["nextCursor"].as_str().assert_value())),
        )
        .await
        .assert_value();
    assert_eq!(at_head["events"], json!([]));
    assert_eq!(at_head["complete"], true);
}

#[tokio::test]
async fn invalid_cursors_and_missing_history_are_explicit() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    for value in ["bad", "v2:9999"] {
        let error = fixture
            .service
            .page(&fixture.id, Some(Cursor::new(value)))
            .await
            .err()
            .assert_value();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }
    let missing = RunId::new(uuid::Uuid::now_v7().to_string());
    std::fs::create_dir_all(fixture.root.join("runs").join(missing.as_str())).assert_value();
    let result = fixture.service.list(None).await.assert_value();
    let unavailable = result["runs"]
        .as_array()
        .assert_value()
        .iter()
        .find(|run| run["runId"] == missing.as_str())
        .assert_value();
    assert_eq!(unavailable["historyAvailable"], false);
    assert!(
        !fixture
            .root
            .join("runs")
            .join(missing.as_str())
            .join("runs.sqlite3")
            .exists()
    );
    assert!(parse_id("../../private".into()).is_err());
}

#[tokio::test]
async fn replay_detects_missing_event_and_never_touches_operator_diagnostics() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    let directory = fixture.root.join("runs").join(fixture.id.as_str());
    std::fs::write(
        directory.join("operator-diagnostics.jsonl"),
        "private operator detail",
    )
    .assert_value();
    let connection = rusqlite::Connection::open(directory.join("runs.sqlite3")).assert_value();
    connection
        .execute("DELETE FROM v2_run_events WHERE sequence=1", [])
        .assert_value();
    let error = fixture
        .service
        .page(&fixture.id, None)
        .await
        .err()
        .assert_value();
    assert_eq!(error.code, "history_gap");
    let detail = fixture.service.detail(&fixture.id).await.assert_value();
    assert!(!detail.to_string().contains("private operator detail"));
}

#[tokio::test]
async fn read_only_ledger_cannot_mutate_or_create_history() {
    let fixture = Fixture::new().await;
    let observer = fixture.service.open(&fixture.id).await.assert_value();
    assert!(
        observer
            .append(&fixture.id, vec![RunEvent::RunStarted])
            .await
            .is_err()
    );
    assert_eq!(
        fixture
            .ledger
            .get(&fixture.id)
            .await
            .assert_value()
            .assert_value()
            .snapshot
            .cursor
            .as_str(),
        "v2:0"
    );
    let missing = fixture.root.join("absent.sqlite3");
    assert!(SqliteRunLedger::open_read_only(&missing).is_err());
    assert!(!missing.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_run_and_ledger_are_not_followed() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new().await;
    let alias = RunId::new(uuid::Uuid::now_v7().to_string());
    symlink(
        fixture.root.join("runs").join(fixture.id.as_str()),
        fixture.root.join("runs").join(alias.as_str()),
    )
    .assert_value();
    assert!(fixture.service.detail(&alias).await.is_err());
    let listed = fixture.service.list(None).await.assert_value();
    assert_eq!(listed["runs"].as_array().assert_value().len(), 1);
    let directory = fixture.root.join("runs").join(fixture.id.as_str());
    std::fs::rename(
        directory.join("runs.sqlite3"),
        directory.join("real.sqlite3"),
    )
    .assert_value();
    symlink(
        directory.join("real.sqlite3"),
        directory.join("runs.sqlite3"),
    )
    .assert_value();
    assert!(fixture.service.detail(&fixture.id).await.is_err());
}

#[tokio::test]
async fn history_routes_keep_json_errors_and_the_local_browser_boundary() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .assert_value();
    let authority = listener.local_addr().assert_value().to_string();
    let app = super::super::router(
        UiState::new(
            crate::native_v2_cli::LocalRunProfileStore::new(fixture.root.join("profiles")),
            fixture.service.clone(),
            &format!("http://{authority}"),
            "local",
        )
        .assert_value(),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.assert_value();
    });
    let client = reqwest::Client::new();
    let url = format!(
        "http://{authority}/ui/api/runs/{}/history",
        fixture.id.as_str()
    );
    let response = client.get(&url).send().await.assert_value();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let response: Value = response.json().await.assert_value();
    assert_eq!(
        response["events"][1]["event"]["reference"]["execution"],
        "1"
    );
    for query in [
        "after=v2%3A999",
        "after=bad",
        "unknown=value",
        "after=v2%3A0&after=v2%3A1",
    ] {
        let response = client
            .get(format!("{url}?{query}"))
            .send()
            .await
            .assert_value();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response: Value = response.json().await.assert_value();
        assert_eq!(response["code"], "invalid_cursor");
    }
    let response = client
        .get(&url)
        .header("origin", "https://untrusted.example")
        .send()
        .await
        .assert_value();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = client
        .get(format!("http://{authority}/ui/api/runs"))
        .send()
        .await
        .assert_value();
    assert_eq!(response.status(), StatusCode::OK);
    task.abort();
}

#[tokio::test]
async fn list_cursor_is_stable_when_new_runs_arrive() {
    let fixture = Fixture::new().await;
    let mut ids = vec![fixture.id.clone()];
    for _ in 0..LIST_PAGE_SIZE + 2 {
        let id = RunId::new(uuid::Uuid::now_v7().to_string());
        std::fs::create_dir_all(fixture.root.join("runs").join(id.as_str())).assert_value();
        ids.push(id);
    }
    let first = fixture.service.list(None).await.assert_value();
    assert_eq!(
        first["runs"].as_array().assert_value().len(),
        LIST_PAGE_SIZE
    );
    let after = parse_id(first["nextCursor"].as_str().assert_value().to_owned())
        .ok()
        .assert_value();
    let fresh = RunId::new(uuid::Uuid::now_v7().to_string());
    std::fs::create_dir_all(fixture.root.join("runs").join(fresh.as_str())).assert_value();
    let second = fixture.service.list(Some(after)).await.assert_value();
    assert_eq!(second["runs"].as_array().assert_value().len(), 3);
    assert!(second["nextCursor"].is_null());
    let combined = first["runs"]
        .as_array()
        .assert_value()
        .iter()
        .chain(second["runs"].as_array().assert_value())
        .map(|run| run["runId"].as_str().assert_value())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(combined.len(), ids.len());
    assert!(!combined.contains(fresh.as_str()));
}
