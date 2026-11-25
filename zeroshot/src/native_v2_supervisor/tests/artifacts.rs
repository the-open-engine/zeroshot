use super::*;
#[path = "artifacts/receipt_sizes.rs"]
mod receipt_sizes;
use crate::native_v2_files::{DiskFileStore, FileAccess, SingleAccountFiles, TargetFileHost};
use crate::native_v2_files::publication::{ManagedFilePublisher, RunFilePublisher};
use openengine_cluster_protocol::{FilePath, FileScope, PublishedFiles, UnixTimestampMillis};
use openengine_cluster_server::identity::{
    BindingAttributes, ConnectionIdentity, ConnectionIdentityConfig, PrincipalId, TenantId,
};

struct Writer {
    inner: Arc<FakeDriver>,
    workspace: std::path::PathBuf,
}
#[async_trait]
impl NodeDriver for Writer {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        std::fs::create_dir_all(self.workspace.join("report/nested")).unwrap();
        std::fs::write(
            self.workspace.join("report/nested/current"),
            "written by the worker",
        )
        .unwrap();
        self.inner.run(invocation, control).await
    }
}

struct Publisher {
    inner: ManagedFilePublisher,
    ledger: Arc<FakeRunLedger>,
    sessions: Arc<FakeSessionFactory>,
    fail_after_commit: AtomicBool,
}
#[async_trait]
impl RunFilePublisher for Publisher {
    async fn publish(
        &self,
        request: crate::native_v2_files::publication::FilePublication,
    ) -> std::io::Result<PublishedFiles> {
        let snapshot = stored_run(&self.ledger).await.snapshot;
        assert!(snapshot.primary_graph.is_none());
        assert!(snapshot.suspension.is_none());
        assert_eq!(
            self.sessions.closed.load(Ordering::SeqCst),
            0,
            "publication must precede runner closure"
        );
        let files = self.inner.publish(request).await?;
        if self.fail_after_commit.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::other(
                "simulated controller loss after storage commit",
            ));
        }
        Ok(files)
    }
}
fn state() -> Value {
    json!({"kind":"record","fields":{"text":{"required":true,"type":{"kind":"string"}}}})
}
fn ask() -> Value {
    json!({"kind":"verifier","name":"approval","worker":crate::native_v2_request::ASK_WORKER_REF,
        "input":crate::native_v2_request::input_schema().assert_value(),"output":{"kind":"record","fields":{}},
        "signals":{"decision":["approved","rejected"]},"diagnostic":crate::native_v2_wait::diagnostic_schema().assert_value(),
        "inputBindings":[{"target":["text"],"value":{"source":"state","path":["text"]}}],"writeBindings":[],"attempts":1,
        "artifacts":{"report":{"kind":"literal","path":"report"}}})
}
fn identity() -> ConnectionIdentity {
    ConnectionIdentity::new(ConnectionIdentityConfig {
        principal: PrincipalId::new("local"),
        tenant: TenantId::new("local"),
        issued_at_ms: None,
        expires_at_ms: u64::MAX,
        binding_attributes: BindingAttributes::default(),
    })
}
fn attach(
    harness: &mut Harness,
    root: &std::path::Path,
    fail_after_commit: bool,
) -> Arc<DiskFileStore> {
    std::fs::create_dir_all(root.join("workspace/report/empty")).unwrap();
    std::fs::write(root.join("workspace/report/file"), "captured").unwrap();
    let store = Arc::new(
        DiskFileStore::open(
            root.join("store"),
            "test".to_owned(),
            Arc::new(SingleAccountFiles {
                principal: "local".to_owned(),
                tenant: "local".to_owned(),
            }),
        )
        .unwrap(),
    );
    harness.supervisor = harness
        .supervisor
        .clone()
        .with_file_publisher(Some(Arc::new(Publisher {
            inner: ManagedFilePublisher {
                workspace: root.join("workspace"),
                store: store.clone(),
                identity: identity(),
            },
            ledger: harness.ledger.clone(),
            sessions: harness.sessions.clone(),
            fail_after_commit: AtomicBool::new(fail_after_commit),
        })));
    store
}

#[tokio::test]
async fn request_capture_replays_the_committed_receipt_before_suspension_and_redacts_status() {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let mut harness = harness_with_session_scope(
        graph(
            sequence(vec![step("writer", 1000), ask(), succeed("done")], state()),
            state(),
        ),
        json!({"text":"Review"}),
        FakeDriver::default(),
        SessionScope::NodeInstance,
    )
    .await;
    let (supervisor, _) = super::checkpoint_tests::RecordingCheckpointStore::attach(&harness);
    harness.supervisor = supervisor;
    let store = attach(&mut harness, temp.path(), true);
    std::fs::remove_dir_all(temp.path().join("workspace/report")).unwrap();
    let admitted = stored_run(&harness.ledger).await.admitted;
    harness.supervisor.runner = Arc::new(
        NativeNodeRunner::new(
            &admitted,
            Arc::new(Writer {
                inner: harness.driver.clone(),
                workspace: temp.path().join("workspace"),
            }),
            harness.sessions.clone(),
        )
        .unwrap(),
    );
    assert!(matches!(
        harness.supervisor.drive().await,
        Err(NativeV2SupervisorError::FilePublication(_))
    ));
    let intent = harness
        .supervisor
        .snapshot()
        .await
        .unwrap()
        .pending_request
        .unwrap();
    let openengine_cluster_protocol::RunSuspension::Request {
        request: pending, ..
    } = intent.suspension
    else {
        panic!()
    };
    std::fs::remove_dir_all(temp.path().join("workspace")).unwrap();
    // Recovery retries the frozen request ID; workspace loss cannot recapture different bytes.
    let sessions = Arc::new(FakeSessionFactory::default());
    let runner = Arc::new(
        NativeNodeRunner::new(&admitted, harness.driver.clone(), sessions.clone()).unwrap(),
    );
    let mut recovered = NativeV2Supervisor::new(
        harness.supervisor.run_id.clone(),
        harness.ledger.clone(),
        runner,
        harness.supervisor.environment.clone(),
    )
    .with_file_publisher(Some(Arc::new(Publisher {
        inner: ManagedFilePublisher {
            workspace: temp.path().join("workspace"),
            store: store.clone(),
            identity: identity(),
        },
        ledger: harness.ledger.clone(),
        sessions,
        fail_after_commit: AtomicBool::new(false),
    })));
    recovered.checkpoints = harness.supervisor.checkpoints.clone();
    harness.supervisor = recovered;
    assert!(matches!(
        harness.supervisor.drive().await,
        Err(NativeV2SupervisorError::Suspended)
    ));
    let snapshot = harness.supervisor.snapshot().await.unwrap();
    let openengine_cluster_protocol::RunSuspension::Request { request, .. } =
        snapshot.suspension.unwrap()
    else {
        panic!()
    };
    assert_eq!(request.id, pending.id);
    assert_eq!(request.artifacts.len(), 1);
    assert_eq!(
        store
            .list(
                &FileAccess {
                    target: "test",
                    identity: &identity()
                },
                FileScope::Run {
                    run_id: snapshot.run_id.clone()
                },
                &FilePath::new("").unwrap()
            )
            .unwrap()
            .len(),
        1
    );
    let public = crate::native_v2_observability::NativeV2Observability::new(harness.ledger.clone())
        .status(openengine_cluster_protocol::RunStatusParams {
            run_id: snapshot.run_id.clone(),
        })
        .await
        .unwrap();
    let openengine_cluster_protocol::RunStatus::Suspended {
        suspension: openengine_cluster_protocol::RunSuspension::Request { request, .. },
        ..
    } = public.status
    else {
        panic!()
    };
    assert!(request.artifacts.is_empty());
    let inbox = crate::v2_run_ledger::inbox::show(
        harness.ledger.as_ref(),
        &snapshot.run_id,
        UnixTimestampMillis::new(1).unwrap(),
    )
    .await
    .unwrap();
    assert!(
        serde_json::to_value(inbox)
            .unwrap()
            .to_string()
            .contains("requests/")
    );
    assert_eq!(harness.driver.starts("approval"), 0);
}

#[tokio::test]
async fn final_capture_precedes_primary_result_and_terminal_output_remains_independent() {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let mut done = succeed("done");
    done["artifacts"] = json!({"report":{"kind":"literal","path":"report"}});
    let mut harness = harness_with_session_scope(
        graph(
            sequence(vec![step("writer", 1000), done], null_type()),
            null_type(),
        ),
        Value::Null,
        FakeDriver::default(),
        SessionScope::NodeInstance,
    )
    .await;
    attach(&mut harness, temp.path(), false);
    assert_eq!(
        harness.supervisor.drive().await.unwrap(),
        TerminalResult::Succeeded {
            output: Value::Null
        }
    );
    let snapshot = harness.supervisor.snapshot().await.unwrap();
    assert_eq!(snapshot.artifacts.len(), 1);
    assert!(snapshot.primary_graph.is_some());
    assert_eq!(harness.sessions.closed.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(temp.path().join("workspace")).unwrap();
    assert_eq!(
        harness.supervisor.drive().await.unwrap(),
        TerminalResult::Succeeded {
            output: Value::Null
        }
    );
}

#[tokio::test]
async fn missing_publication_after_validation_is_runtime_failure_without_model_correction() {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let mut done = succeed("done");
    done["artifacts"] = json!({"report":{"kind":"literal","path":"report"}});
    let mut harness = harness(
        graph(sequence(vec![done], null_type()), null_type()),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    let store = attach(&mut harness, temp.path(), false);
    std::fs::rename(
        temp.path().join("workspace/report"),
        temp.path().join("moved"),
    )
    .unwrap();
    let error = harness.supervisor.drive().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("artifacts.report path \"report\"")
    );
    assert!(
        harness
            .supervisor
            .snapshot()
            .await
            .unwrap()
            .primary_graph
            .is_none()
    );
    harness
        .supervisor
        .fail_runtime(&error.to_string(), || {})
        .await
        .unwrap();
    assert!(
        matches!(harness.supervisor.snapshot().await.unwrap().terminal, Some(TerminalResult::Failed { reason, .. }) if reason.as_str()=="runtime_failed")
    );
    assert!(
        store
            .list(
                &FileAccess {
                    target: "test",
                    identity: &identity()
                },
                FileScope::Run {
                    run_id: harness.supervisor.run_id.clone()
                },
                &FilePath::new("").unwrap()
            )
            .unwrap()
            .is_empty()
    );
}

async fn production_restart_after_committed_publication(request: bool, committed: bool) {
    use crate::native_v2_hosting::{ProductionHostingConfig, ProductionTargetControllerFactory};
    use crate::v2_run_ledger::{CreateRun, RunLedger, sqlite::SqliteRunLedger};
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = json!({"kind":"record","fields":{
        "text":{"required":true,"type":{"kind":"string"}},
        "artifact":{"required":true,"type":{"kind":"filepath"}}
    }});
    let artifacts = json!({
        "literal":{"kind":"literal","path":"report"},
        "bound":{"kind":"binding","value":{"source":"state","path":["artifact"]}}
    });
    let mut breakpoint = if request { ask() } else { succeed("done") };
    breakpoint["artifacts"] = artifacts;
    let mut children = vec![step("writer", 1000), breakpoint];
    if request {
        children.push(succeed("done"));
    }
    let mut harness = harness_with_session_scope(
        graph(sequence(children, state.clone()), state),
        json!({"text":"Review", "artifact":"report"}),
        FakeDriver::default(),
        SessionScope::NodeInstance,
    )
    .await;
    if request {
        let (supervisor, _) = super::checkpoint_tests::RecordingCheckpointStore::attach(&harness);
        harness.supervisor = supervisor;
    }
    let store = attach(&mut harness, temp.path(), true);
    assert!(matches!(
        harness.supervisor.drive().await,
        Err(NativeV2SupervisorError::FilePublication(_))
    ));
    let stored = stored_run(&harness.ledger).await;
    assert!(stored.snapshot.primary_graph.is_none());
    assert!(stored.snapshot.suspension.is_none());
    let intent = pending_file_publication(&stored).unwrap().unwrap();
    let files = store
        .publication_receipt(
            &FileAccess {
                target: "test",
                identity: &identity(),
            },
            &intent,
        )
        .unwrap()
        .unwrap();
    assert_eq!(files.len(), 2);
    // Copy the actual supervisor-produced cut into the same SQLite layout the production factory opens.
    let root = temp.path().join("production");
    std::fs::create_dir(&root).unwrap();
    let ledger = Arc::new(SqliteRunLedger::open(root.join("runs.sqlite3")).unwrap());
    ledger
        .create_or_get(CreateRun {
            run_id: stored.snapshot.run_id.clone(),
            submission_key: stored.submission_key.clone(),
            submission_digest: stored.submission_digest.clone(),
            admitted: stored.admitted.clone(),
        })
        .await
        .unwrap();
    let events = harness
        .ledger
        .snapshot_and_tail(&stored.snapshot.run_id, None)
        .await
        .unwrap()
        .events;
    assert_eq!(events.last().unwrap().cursor, stored.snapshot.cursor);
    ledger
        .append(
            &stored.snapshot.run_id,
            events.into_iter().map(|event| event.event).collect(),
        )
        .await
        .unwrap();
    std::fs::remove_dir_all(temp.path().join("workspace")).unwrap();
    let store = if committed {
        store
    } else {
        Arc::new(
            DiskFileStore::open(
                temp.path().join("uncommitted-store"),
                "test".to_owned(),
                Arc::new(SingleAccountFiles {
                    principal: "local".to_owned(),
                    tenant: "local".to_owned(),
                }),
            )
            .unwrap(),
        )
    };
    let config = ProductionHostingConfig {
        storage_root: root,
        file_store: Some((store.clone(), identity())),
        // Any provider launch or checkout during receipt recovery must fail this fixture.
        git_program: temp.path().join("must-not-run-git"),
        codex_executable: temp.path().join("must-not-run-codex"),
        ..ProductionHostingConfig::default()
    };
    for _ in 0..2 {
        let controller = ProductionTargetControllerFactory::new(config.clone())
            .create_controller()
            .await
            .unwrap();
        let recovered = ledger
            .get(&stored.snapshot.run_id)
            .await
            .unwrap()
            .unwrap()
            .snapshot;
        if !committed {
            assert!(
                matches!(recovered.terminal, Some(TerminalResult::Failed { reason, .. }) if reason.as_str() == "runtime_lost")
            );
            assert!(recovered.suspension.is_none());
            assert!(recovered.artifacts.is_empty());
        } else if request {
            assert_eq!(recovered.phase, RunPhase::Suspended);
            assert!(recovered.terminal.is_none());
            assert!(recovered.pending_request.is_none());
            let openengine_cluster_protocol::RunSuspension::Request {
                request: question, ..
            } = recovered.suspension.unwrap()
            else {
                panic!()
            };
            let crate::native_v2_files::FilePublicationOwner::Request { request: id, .. } =
                &intent.owner
            else {
                panic!()
            };
            assert_eq!(question.id.as_str(), id.as_str());
            assert_eq!(question.artifacts, files);
            let inbox = controller
                .inbox_show(openengine_cluster_protocol::InboxShowParams {
                    run_id: stored.snapshot.run_id.clone(),
                })
                .await
                .unwrap();
            assert!(serde_json::to_string(&inbox).unwrap().contains("requests/"));
            assert!(
                !serde_json::to_string(&controller.list().await.unwrap())
                    .unwrap()
                    .contains("requests/")
            );
        } else {
            assert_eq!(
                recovered.terminal,
                Some(TerminalResult::Succeeded {
                    output: Value::Null
                })
            );
            assert_eq!(recovered.artifacts, files);
            assert!(matches!(
                recovered.primary_graph.unwrap().result,
                TerminalResult::Succeeded { .. }
            ));
        }
        assert_eq!(
            store
                .list(
                    &FileAccess {
                        target: "test",
                        identity: &identity()
                    },
                    openengine_cluster_protocol::FileScope::Run {
                        run_id: stored.snapshot.run_id.clone()
                    },
                    &FilePath::new("").unwrap()
                )
                .unwrap()
                .len(),
            if committed { 2 } else { 0 }
        );
        if committed {
            for file in files.values() {
                let openengine_cluster_protocol::PublishedFile::Managed { address, .. } = file
                else {
                    panic!()
                };
                let mut bytes = Vec::new();
                store
                    .download(
                        &FileAccess {
                            target: "test",
                            identity: &identity(),
                        },
                        &openengine_cluster_protocol::FileSelection {
                            address: address.clone(),
                            member: FilePath::new("file").unwrap(),
                        },
                        &mut bytes,
                    )
                    .unwrap();
                let out = tempfile::tempdir().unwrap();
                let mut transfer = tempfile::tempfile().unwrap();
                std::io::Write::write_all(&mut transfer, &bytes).unwrap();
                std::io::Seek::rewind(&mut transfer).unwrap();
                crate::native_v2_files::unpack(&mut transfer, &out.path().join("saved")).unwrap();
                assert_eq!(
                    std::fs::read(out.path().join("saved")).unwrap(),
                    b"captured"
                );
            }
        }
    }
    assert_eq!(harness.driver.starts("writer"), 1);
}

#[tokio::test]
async fn production_factory_recovers_committed_request_before_runtime_loss_settlement() {
    production_restart_after_committed_publication(true, true).await;
}

#[tokio::test]
async fn production_factory_recovers_committed_final_before_runtime_loss_settlement() {
    production_restart_after_committed_publication(false, true).await;
}

#[tokio::test]
async fn production_factory_never_recaptures_an_uncommitted_breakpoint() {
    production_restart_after_committed_publication(true, false).await;
    production_restart_after_committed_publication(false, false).await;
}
