use super::*;
use crate::native_v2_cli::CliSubscription;
use crate::native_v2_runner::test_support;
use crate::v2_run_ledger::{CreateRun, RunEvent, SafeLogLine, SafeLogStream, sqlite_crash};
use openengine_cluster_protocol::{ControllerState, UnixTimestampMillis, WorkerOutcome};
use std::collections::BTreeMap;
use std::process::{Command, Stdio};

struct Fixture {
    root: tempfile::TempDir,
    backend: LocalCliBackend,
    id: RunId,
    execution: openengine_cluster_protocol::ExecutionRef,
}

impl Fixture {
    async fn new(finished: bool, lost: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let backend = LocalCliBackend::new(
            root.path().to_owned(),
            "unused".into(),
            root.path().to_owned(),
            "git".into(),
        );
        let id = RunId::new(uuid::Uuid::now_v7().to_string());
        let storage = backend.create_run_storage(&id).unwrap();
        let ledger = SqliteRunLedger::open(storage.join("runs.sqlite3")).unwrap();
        ledger
            .create_or_get(CreateRun {
                run_id: id.clone(),
                submission_key: IdempotencyKey::new(id.as_str()).unwrap(),
                submission_digest: Sha256Digest::new("a".repeat(64)).unwrap(),
                admitted: test_support::admitted(),
            })
            .await
            .unwrap();
        let reference = test_support::request(id.as_str(), "worker", (1, 1))
            .invocation
            .reference;
        let mut events = vec![
            RunEvent::RunStarted,
            test_support::node_started(&reference, serde_json::Value::Null),
        ];
        // Cross bounded ledger pages, including pages with no watch status changes.
        events.extend((0..300).map(|index| RunEvent::SafeLog {
            execution: Some(reference.execution),
            timestamp: UnixTimestampMillis::MIN,
            stream: SafeLogStream::Output,
            line: SafeLogLine::new(format!("recorded {index}")).unwrap(),
            diagnostic: None,
        }));
        if finished {
            events.push(RunEvent::NodeCompleted {
                completion: crate::native_v2_contract::NodeCompletion {
                    reference,
                    outcome: WorkerOutcome::Verified {
                        output: serde_json::Value::Null,
                        artifacts: vec![],
                    },
                },
            });
            events.push(RunEvent::Terminal {
                result: TerminalResult::Succeeded {
                    output: serde_json::Value::Null,
                },
            });
        }
        ledger.append(&id, events).await.unwrap();
        let observed =
            crate::native_v2_observability::NativeV2Observability::new(Arc::new(ledger.clone()));
        let execution = observed
            .status(RunStatusParams { run_id: id.clone() })
            .await
            .unwrap();
        let execution = match execution.status {
            RunStatus::Running {
                active_executions, ..
            } => active_executions[0].execution.clone(),
            _ => openengine_cluster_protocol::ExecutionRef::new("finished-execution").unwrap(),
        };
        drop(ledger);
        // Even a finished run with stale readiness must bypass its controller entirely.
        let paths = backend.paths(&id).unwrap();
        let lease = ControllerLease::acquire(paths.lease()).unwrap();
        let pid = if lost {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .arg("--list")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let pid = child.id();
            child.wait().unwrap();
            pid
        } else {
            std::process::id()
        };
        let mut file = private_file(&paths.ready(), FileAccess::CreateNew).unwrap();
        use std::io::Write;
        file.write_all(
            &serde_json::to_vec(
                &crate::native_v2_portable_controller::PortableControllerReady {
                    kind: "zeroshot.portable-controller-ready/v1".into(),
                    run_id: id.clone(),
                    socket: paths.socket(),
                    pid,
                    lease_identity: Some(lease.identity()),
                },
            )
            .unwrap(),
        )
        .unwrap();
        drop(file);
        drop(lease);
        Self {
            root,
            backend,
            id,
            execution,
        }
    }
}

#[derive(Debug, PartialEq)]
struct Entry {
    bytes: Option<Vec<u8>>,
    modified: std::time::SystemTime,
    permissions: std::fs::Permissions,
}

fn tree(path: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut entries = BTreeMap::new();
    let metadata = std::fs::metadata(path).unwrap();
    if metadata.is_dir() {
        for child in std::fs::read_dir(path).unwrap() {
            entries.extend(tree(&child.unwrap().path()));
        }
    }
    entries.insert(
        path.to_owned(),
        Entry {
            bytes: metadata.is_file().then(|| std::fs::read(path).unwrap()),
            modified: metadata.modified().unwrap(),
            permissions: metadata.permissions(),
        },
    );
    entries
}

fn readonly(path: &Path) {
    let metadata = std::fs::metadata(path).unwrap();
    if metadata.is_dir() {
        for child in std::fs::read_dir(path).unwrap() {
            readonly(&child.unwrap().path());
        }
    }
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(if metadata.is_dir() { 0o500 } else { 0o400 });
    }
    #[cfg(windows)]
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions).unwrap();
}

async fn drain<E: Send>(
    subscription: &mut ChannelSubscription<E>,
    finished: bool,
    id: &RunId,
) -> usize {
    let mut count = 0;
    loop {
        match subscription.next().await {
            Ok(Some(CliSubscriptionItem::Event(_))) => count += 1,
            Ok(Some(CliSubscriptionItem::Closed {
                reason: SubscriptionCloseReason::Done,
            })) if finished => return count,
            Err(error) if !finished => {
                let message = error.to_string();
                assert!(message.contains(&format!("zeroshot resume {}", id.as_str())));
                assert!(message.contains(&format!("zeroshot force-stop {}", id.as_str())));
                return count;
            }
            _ => panic!("unexpected replay termination"),
        }
    }
}

#[tokio::test]
async fn inspection_leaves_read_only_running_lost_and_finished_directories_unchanged() {
    for (finished, lost) in [(false, false), (false, true), (true, true)] {
        let fixture = Fixture::new(finished, lost).await;
        sqlite_crash::interrupt(&fixture.backend.paths(&fixture.id).unwrap().ledger());
        let original = tree(fixture.root.path());
        readonly(fixture.root.path());
        let before = tree(fixture.root.path());
        let result = tokio::time::timeout(Duration::from_secs(20), async {
            let status = fixture
                .backend
                .status_local(RunStatusParams {
                    run_id: fixture.id.clone(),
                })
                .await
                .unwrap();
            assert_eq!(
                matches!(status.status, RunStatus::Finished { .. }),
                finished
            );
            if finished {
                assert!(status.controller.is_none());
            } else {
                assert_eq!(
                    status.controller.unwrap().state,
                    if lost {
                        ControllerState::Lost
                    } else {
                        ControllerState::Unknown
                    }
                );
            }
            let listed = fixture.backend.list_local().await.unwrap();
            assert_eq!(listed.runs.len(), 1);
            assert_eq!(listed.runs[0].controller.is_none(), finished);
            let checkpoints = fixture
                .backend
                .run_checkpoints(
                    None,
                    openengine_cluster_protocol::RunCheckpointsParams {
                        run_id: fixture.id.clone(),
                        after: None,
                        limit: None,
                    },
                )
                .await
                .unwrap();
            assert_eq!(checkpoints.controller.is_none(), finished);
            let mut watch = fixture
                .backend
                .inspect_watch(RunWatchParams {
                    run_id: fixture.id.clone(),
                    from_cursor: None,
                })
                .await
                .unwrap();
            assert!(drain(&mut watch, finished, &fixture.id).await >= 2);
            let mut logs = fixture
                .backend
                .inspect_logs(RunLogsParams {
                    run_id: fixture.id.clone(),
                    from_cursor: None,
                    execution: None,
                })
                .await
                .unwrap();
            assert_eq!(drain(&mut logs, finished, &fixture.id).await, 300);
            let attach = fixture
                .backend
                .inspect_attach(RunAttachParams {
                    run_id: fixture.id.clone(),
                    execution: fixture.execution.clone(),
                })
                .await;
            if finished {
                assert!(attach.is_err());
            } else {
                assert_eq!(drain(&mut attach.unwrap(), false, &fixture.id).await, 300);
            }
        })
        .await;
        let after = tree(fixture.root.path());
        // Restore permissions before assertions so a failure cannot strand the fixture.
        for (path, entry) in original {
            std::fs::set_permissions(path, entry.permissions).unwrap();
        }
        result.unwrap();
        assert_eq!(before, after);
    }
}

#[tokio::test]
async fn force_of_a_recorded_terminal_needs_no_controller() {
    let fixture = Fixture::new(true, false).await;
    let before = tree(fixture.root.path());
    let forced = fixture
        .backend
        .force_once(RunForceParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(
        forced.status,
        RunStatus::Finished {
            terminal_result: TerminalResult::Succeeded { .. },
            ..
        }
    ));
    assert_eq!(before, tree(fixture.root.path()));
}

#[tokio::test]
async fn controls_refuse_a_live_pid_or_held_lease_and_only_resume_confirms_loss() {
    let live = Fixture::new(false, false).await;
    let before = tree(live.root.path());
    assert!(
        live.backend
            .confirm_loss_for_resume(&live.id)
            .await
            .is_err()
    );
    assert!(
        live.backend
            .force_once(RunForceParams {
                run_id: live.id.clone()
            })
            .await
            .is_err()
    );
    assert_eq!(before, tree(live.root.path()));
    let lost = Fixture::new(false, true).await;
    let lease = ControllerLease::acquire(lost.backend.paths(&lost.id).unwrap().lease()).unwrap();
    assert!(
        lost.backend
            .confirm_loss_for_resume(&lost.id)
            .await
            .is_err()
    );
    drop(lease);
    lost.backend
        .confirm_loss_for_resume(&lost.id)
        .await
        .unwrap();
    let status = lost
        .backend
        .status_local(RunStatusParams {
            run_id: lost.id.clone(),
        })
        .await
        .unwrap();
    assert!(status.controller.is_none());
    assert!(
        matches!(status.status, RunStatus::Finished { terminal_result: TerminalResult::Failed { reason, .. }, .. } if reason.as_str() == "runtime_lost")
    );
}

async fn reconcile(fixture: &Fixture, force: bool) -> Result<(), NativeV2CliError> {
    if force {
        fixture
            .backend
            .force_once(RunForceParams {
                run_id: fixture.id.clone(),
            })
            .await
            .map(|_| ())
    } else {
        fixture.backend.confirm_loss_for_resume(&fixture.id).await
    }
}

async fn assert_recovery_can_retry(fixture: &Fixture, force: bool, ready: &[u8]) {
    let paths = fixture.backend.paths(&fixture.id).unwrap();
    assert_eq!(std::fs::read(paths.ready()).unwrap(), ready);
    let status = fixture
        .backend
        .status_local(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(status.status, RunStatus::Running { .. }));
    assert_eq!(status.controller.unwrap().state, ControllerState::Lost);

    // A failed recovery released its claim; an intervening owner must still fence retries.
    let lease = ControllerLease::acquire(paths.lease()).unwrap();
    assert!(reconcile(fixture, force).await.is_err());
    assert_eq!(std::fs::read(paths.ready()).unwrap(), ready);
    drop(lease);

    reconcile(fixture, force).await.unwrap();
    let status = fixture
        .backend
        .read_recorded_status(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap();
    assert!(
        matches!(status.status, RunStatus::Finished { terminal_result: TerminalResult::Failed { reason, .. }, .. } if reason.as_str() == "runtime_lost")
    );
    if !force {
        assert!(!paths.ready().exists());
    }
}

#[tokio::test]
async fn controls_preserve_readiness_when_ledger_is_read_only_and_retry() {
    for force in [false, true] {
        let fixture = Fixture::new(false, true).await;
        let paths = fixture.backend.paths(&fixture.id).unwrap();
        let ready = std::fs::read(paths.ready()).unwrap();
        let original = std::fs::metadata(paths.ledger()).unwrap().permissions();
        let mut permissions = original.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(paths.ledger(), permissions).unwrap();
        let result = reconcile(&fixture, force).await;
        std::fs::set_permissions(paths.ledger(), original).unwrap();
        assert!(result.is_err());
        assert_recovery_can_retry(&fixture, force, &ready).await;
    }
}

#[tokio::test]
async fn controls_preserve_readiness_when_loss_append_fails_and_retry() {
    for force in [false, true] {
        let fixture = Fixture::new(false, true).await;
        let paths = fixture.backend.paths(&fixture.id).unwrap();
        let ready = std::fs::read(paths.ready()).unwrap();
        let connection = rusqlite::Connection::open(paths.ledger()).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_recovery BEFORE INSERT ON v2_run_events
                 BEGIN SELECT RAISE(ABORT, 'recovery persistence refused'); END;",
            )
            .unwrap();
        assert!(reconcile(&fixture, force).await.is_err());
        connection
            .execute_batch("DROP TRIGGER reject_recovery")
            .unwrap();
        drop(connection);
        assert_recovery_can_retry(&fixture, force, &ready).await;
    }
}
