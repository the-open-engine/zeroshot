use super::*;
use crate::native_v2_files::publication::FilePublication;
use crate::native_v2_hosting::{ProductionHostingConfig, ProductionTargetControllerFactory};
use crate::v2_run_ledger::{
    CreateRun, MAX_EVENT_BYTES, PrimaryGraphCompletion, sqlite::SqliteRunLedger,
};
use openengine_cluster_protocol::{FieldName, PublishedFile, RunSuspension, MAX_REQUEST_RESPONSE_BYTES};

fn field(index: usize) -> String {
    format!("f{index:04}{}", "x".repeat(122))
}

fn receipt(count: usize) -> PublishedFiles {
    (0..count)
        .map(|index| {
            let field = field(index);
            (
                FieldName::new(&field).unwrap(),
                PublishedFile::Managed {
                    target: "test".to_owned(),
                    address: openengine_cluster_protocol::FileAddress {
                        scope: FileScope::Run {
                            run_id: RunId::new("run-supervisor-test"),
                        },
                        path: FilePath::new(format!("final/{field}/{}", "x".repeat(128))).unwrap(),
                    },
                },
            )
        })
        .collect()
}

fn final_bytes(files: &PublishedFiles) -> usize {
    serde_json::to_vec(&RunEvent::FinalArtifactsPublished {
        artifacts: files.clone(),
    })
    .unwrap()
    .len()
}

#[tokio::test]
async fn receipt_preflight_uses_the_complete_existing_ledger_and_request_envelopes() {
    let harness = harness(
        graph(succeed("done"), null_type()),
        Value::Null,
        FakeDriver::default(),
    )
    .await;
    let snapshot = stored_run(&harness.ledger).await.snapshot;
    let primary = PrimaryGraphCompletion {
        result: TerminalResult::Succeeded {
            output: Value::Null,
        },
        auxiliary_pending: false,
        delivery_execution: None,
    };
    let final_check =
        super::super::super::publication::final_preflight(&snapshot, &primary).unwrap();
    let mut files = BTreeMap::from([(
        FieldName::new("report").unwrap(),
        PublishedFile::Local {
            root: String::new(),
            path: "report".to_owned(),
        },
    )]);
    let padding = MAX_EVENT_BYTES - final_bytes(&files);
    let PublishedFile::Local { root, .. } = files.values_mut().next().unwrap() else {
        panic!()
    };
    // Escaped path bytes count toward the actual serialized record, not UTF-8 string length.
    *root = "\\".repeat(padding / 2) + &"x".repeat(padding % 2);
    assert_eq!(final_bytes(&files), MAX_EVENT_BYTES);
    final_check(&files).unwrap();
    let PublishedFile::Local { root, .. } = files.values_mut().next().unwrap() else {
        panic!()
    };
    root.push('x');
    assert_eq!(final_bytes(&files), MAX_EVENT_BYTES + 1);
    assert!(final_check(&files).is_err());

    let mut suspension: RunSuspension = serde_json::from_value(json!({
        "reason":"request", "node":"approval", "createdAt":1,
        "request":{"id":"question", "text":"Review", "signals":{"decision":["approved"]},
        "output":{"kind":"record","fields":{}},"expiresAt":1000}
    }))
    .unwrap();
    let request_check =
        super::super::super::publication::request_preflight(&snapshot.run_id, &suspension);
    let RunSuspension::Request { request, .. } = &mut suspension else {
        panic!()
    };
    request.artifacts = files.clone();
    let vector =
        super::super::super::publication::request_vector(&snapshot.run_id, &suspension).unwrap();
    let overhead = serde_json::to_vec(&vector).unwrap().len().max(
        serde_json::to_vec(&RunEvent::Suspended { suspension })
            .unwrap()
            .len(),
    );
    let remaining = MAX_REQUEST_RESPONSE_BYTES - 1024 - overhead;
    let PublishedFile::Local { root, .. } = files.values_mut().next().unwrap() else {
        panic!()
    };
    root.push_str(&"x".repeat(remaining));
    assert!(serde_json::to_vec(&files).unwrap().len() > MAX_EVENT_BYTES);
    request_check(&files).unwrap();
    let PublishedFile::Local { root, .. } = files.values_mut().next().unwrap() else {
        panic!()
    };
    root.push('x');
    assert!(request_check(&files).is_err());
}

#[tokio::test]
async fn publication_size_boundary_replays_and_oversized_sets_cannot_poison_production_restart() {
    let first = final_bytes(&receipt(1));
    let per_file = final_bytes(&receipt(2)) - first;
    let boundary = 1 + (MAX_EVENT_BYTES - first) / per_file;
    assert!(final_bytes(&receipt(boundary)) <= MAX_EVENT_BYTES);
    assert!(final_bytes(&receipt(boundary + 1)) > MAX_EVENT_BYTES);
    for (request, count, accepted) in [
        (false, boundary, true),
        (false, 3000, false),
        (true, 300, true),
        (true, 5000, false),
    ] {
        let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let filename = "x".repeat(128);
        let artifacts: serde_json::Map<String, Value> = (0..count)
            .map(|index| (field(index), json!({"kind":"literal","path":filename})))
            .collect();
        let mut breakpoint = if request { ask() } else { succeed("done") };
        breakpoint["artifacts"] = json!(artifacts);
        let graph = if request {
            graph(
                sequence(vec![breakpoint, succeed("done")], state()),
                state(),
            )
        } else {
            graph(breakpoint, null_type())
        };
        let initial = if request {
            json!({"text":"r".repeat(if accepted { 1 } else { 600_000 })})
        } else {
            Value::Null
        };
        let mut harness = harness(graph, initial, FakeDriver::default()).await;
        let admitted = stored_run(&harness.ledger).await.admitted;
        assert!(
            serde_json::to_vec(&admitted).unwrap().len()
                < crate::v2_run_ledger::MAX_ADMITTED_RUN_BYTES
        );
        if request {
            let (supervisor, _) =
                super::super::checkpoint_tests::RecordingCheckpointStore::attach(&harness);
            harness.supervisor = supervisor;
        }
        let production = temp.path().join("production");
        std::fs::create_dir(&production).unwrap();
        let ledger = Arc::new(SqliteRunLedger::open(production.join("runs.sqlite3")).unwrap());
        ledger
            .create_or_get(CreateRun {
                run_id: harness.supervisor.run_id.clone(),
                submission_key: openengine_cluster_protocol::IdempotencyKey::new("receipt-size")
                    .unwrap(),
                submission_digest: openengine_cluster_protocol::Sha256Digest::new("a".repeat(64))
                    .unwrap(),
                admitted,
            })
            .await
            .unwrap();
        harness.supervisor.ledger = ledger.clone();
        let store = attach(&mut harness, temp.path(), true);
        std::fs::write(temp.path().join("workspace").join(&filename), b"captured").unwrap();
        let result = harness.supervisor.drive().await;
        let Err(NativeV2SupervisorError::FilePublication(error)) = result else {
            panic!("{result:?}")
        };
        assert!(
            error.to_string().contains(if accepted {
                "simulated controller loss"
            } else {
                "artifact publication"
            }),
            "{error}"
        );
        let id = harness.supervisor.run_id.clone();
        let stored = ledger.get(&id).await.unwrap().unwrap();
        assert!(stored.snapshot.primary_graph.is_none() && stored.snapshot.suspension.is_none());
        assert!(stored.snapshot.artifacts.is_empty());
        let intent: FilePublication = pending_file_publication(&stored).unwrap().unwrap();
        let files = store
            .publication_receipt(
                &FileAccess {
                    target: "test",
                    identity: &identity(),
                },
                &intent,
            )
            .unwrap();
        assert_eq!(files.as_ref().map(BTreeMap::len), accepted.then_some(count));
        if !accepted {
            assert!(
                std::fs::read_dir(temp.path().join("store/objects"))
                    .unwrap()
                    .next()
                    .is_none()
            );
            assert!(matches!(
                harness.supervisor.drive().await,
                Err(NativeV2SupervisorError::FilePublication(_))
            ));
        }
        drop(harness);
        std::fs::remove_dir_all(temp.path().join("workspace")).unwrap();
        let config = ProductionHostingConfig {
            storage_root: production,
            file_store: Some((store.clone(), identity())),
            git_program: temp.path().join("must-not-run-git"),
            codex_executable: temp.path().join("must-not-run-codex"),
            ..Default::default()
        };
        for _ in 0..2 {
            let controller = ProductionTargetControllerFactory::new(config.clone())
                .create_controller()
                .await
                .unwrap();
            let snapshot = ledger.get(&id).await.unwrap().unwrap().snapshot;
            if accepted && request {
                assert_eq!(snapshot.phase, RunPhase::Suspended);
                assert_eq!(
                    controller
                        .inbox_show(openengine_cluster_protocol::InboxShowParams {
                            run_id: id.clone()
                        })
                        .await
                        .unwrap()
                        .request
                        .artifacts,
                    files.clone().unwrap()
                );
            } else if accepted {
                assert_eq!(
                    snapshot.terminal,
                    Some(TerminalResult::Succeeded {
                        output: Value::Null
                    })
                );
                assert_eq!(snapshot.artifacts, files.clone().unwrap());
            } else {
                assert!(
                    matches!(snapshot.terminal, Some(TerminalResult::Failed { reason, .. }) if reason.as_str() == "runtime_lost")
                );
            }
            assert_eq!(
                store
                    .list(
                        &FileAccess {
                            target: "test",
                            identity: &identity()
                        },
                        FileScope::Run { run_id: id.clone() },
                        &FilePath::new("").unwrap()
                    )
                    .unwrap()
                    .len(),
                if accepted { count } else { 0 }
            );
            if let Some(files) = &files {
                let PublishedFile::Managed { address, .. } = files.values().next().unwrap() else {
                    panic!()
                };
                store
                    .download(
                        &FileAccess {
                            target: "test",
                            identity: &identity(),
                        },
                        &openengine_cluster_protocol::FileSelection {
                            address: address.clone(),
                            member: FilePath::new("").unwrap(),
                        },
                        &mut Vec::new(),
                    )
                    .unwrap();
            }
        }
    }
}
