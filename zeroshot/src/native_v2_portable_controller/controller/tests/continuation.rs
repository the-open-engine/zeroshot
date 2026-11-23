use super::*;
use crate::native_v2_portable_controller::tests::{NeverDispatched, bootstrap, submission};
use crate::native_v2_runner::test_support;
use crate::v2_run_ledger::{RunEvent, vector_event};
use openengine_cluster_protocol::{
    EnumLabel, NodeName, RunSuspension, RunVectorContent, RunWait, UnixTimestampMillis,
    WorkerOutcome,
};

#[tokio::test]
async fn successor_preparation_inherits_every_vector_and_only_suspended_execution_history() {
    for phase in ["failed", "command", "wait"] {
        let root = tempfile::tempdir().assert_value();
        let runs = private_directory(root.path(), "runs");
        let workspace = private_directory(root.path(), "workspace");
        let marker = workspace.join("edited-after-checkpoint");
        std::fs::write(&marker, "keep current files").assert_value();
        let predecessor = RunId::new("predecessor");
        let source = SqliteRunLedger::open(
            private_directory(&runs, predecessor.as_str()).join("runs.sqlite3"),
        )
        .assert_value();
        source
            .create_or_get(CreateRun {
                run_id: predecessor.clone(),
                submission_key: IdempotencyKey::new("predecessor").assert_value(),
                submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
                admitted: test_support::admitted(),
            })
            .await
            .assert_value();
        let reference = test_support::request(predecessor.as_str(), "worker", (1, 1))
            .invocation
            .reference;
        let floor = 11;
        let mut events = vec![
            RunEvent::NodeInstancesReserved { through: floor },
            RunEvent::RunStarted,
            test_support::node_started(&reference, serde_json::Value::Null),
        ];
        let mut vectors = Vec::new();
        // Cross multiple vector pages; the successor must preserve source IDs and order.
        for _ in 0..205 {
            let event =
                vector_event(&predecessor, RunVectorContent::SuspendRequest {}).assert_value();
            if let RunEvent::Vector { vector } = &event {
                vectors.push(vector.clone());
            }
            events.push(event);
        }
        if phase == "wait" {
            events.push(RunEvent::WaitDeferred {
                reference: reference.clone(),
            });
        } else {
            events.push(RunEvent::NodeCompleted {
                completion: crate::native_v2_contract::NodeCompletion {
                    reference: reference.clone(),
                    outcome: WorkerOutcome::Verified {
                        output: serde_json::Value::Null,
                        artifacts: Vec::new(),
                    },
                },
            });
        }
        events.push(if phase == "failed" {
            RunEvent::Terminal {
                result: TerminalResult::Failed {
                    reason: EnumLabel::new("worker_failed").assert_value(),
                    context: None,
                },
            }
        } else {
            RunEvent::Suspended {
                suspension: if phase == "command" {
                    RunSuspension::Command {
                        node: NodeName::new("next").assert_value(),
                        created_at: UnixTimestampMillis::MIN,
                    }
                } else {
                    RunSuspension::Wait {
                        node: reference.node.clone(),
                        created_at: UnixTimestampMillis::MIN,
                        wait: RunWait {
                            due_at: UnixTimestampMillis::MIN,
                        },
                    }
                },
            }
        });
        source.append(&predecessor, events).await.assert_value();
        let before = source.get(&predecessor).await.assert_value().assert_value();
        let mut bootstrap = bootstrap(
            RunId::new("successor"),
            submission("successor"),
            workspace,
            runs.join("successor"),
        );
        bootstrap.predecessor = Some(predecessor.clone());
        let prepared = prepare_controller_start(&bootstrap).await.assert_value();
        let prepared_runtime = prepare_runtime(
            &prepared,
            &bootstrap,
            async |_, _| Ok::<_, std::io::Error>(PortableRuntime::new(Arc::new(NeverDispatched))),
            tokio::time::Instant::now() + bootstrap.preparation_timeout,
        )
        .await
        .assert_value();
        let runtime = prepared_runtime.runtime.assert_value();
        assert_eq!(runtime.node_instance_floor, floor);
        assert_eq!(&runtime.vector_seed[..vectors.len()], vectors);
        assert_eq!(runtime.vector_seed.len(), vectors.len() + 1);
        let continuation = runtime.vector_seed.last().assert_value();
        assert_eq!(continuation.run_id, bootstrap.run_id);
        assert_eq!(
            continuation.content,
            RunVectorContent::Continuation {
                resumed_from: predecessor.clone()
            }
        );
        if phase == "failed" {
            assert!(
                runtime.execution_seed.is_empty(),
                "default failed-run resume starts fresh"
            );
        } else {
            let expected =
                crate::native_v2_supervisor::continuation_history(&before.snapshot).assert_value();
            assert_eq!(runtime.execution_seed, expected);
            assert_eq!(runtime.execution_seed.len(), 1);
            assert!(matches!(
                runtime.execution_seed[0].state,
                crate::full_v1_reducer::DurableExecutionState::Settled { .. }
            ));
        }
        assert_eq!(
            source.get(&predecessor).await.assert_value().assert_value(),
            before
        );
        assert_eq!(
            std::fs::read_to_string(marker).assert_value(),
            "keep current files"
        );
        assert!(
            prepared.ledger.list().await.assert_value().is_empty(),
            "preparation precedes successor admission"
        );
    }
}

#[tokio::test]
async fn local_controller_accepts_steers_during_suspension_cleanup_and_after_reopen() {
    use crate::native_v2_supervisor::publication_test_store::PublicationStore;
    use openengine_cluster_protocol::{RunSteerParams, RunSuspendParams, SteeringMode};
    use serde_json::json;

    tokio::time::timeout(Duration::from_secs(10), async {
        let root = tempfile::tempdir().assert_value();
        let workspace = private_directory(root.path(), "workspace");
        let storage = private_directory(root.path(), "run");
        let run_id = RunId::new("local-suspended-steering");
        let paths = PortableControllerPaths::new(storage.clone());
        let mut submission = submission("local-suspended-steering");
        let state = json!({"kind":"record","fields":{"durationMs":{"required":true,"type":{"kind":"integer"}}}});
        submission.graph = serde_json::from_value(json!({
            "profile":"openengine.graph.full/v1", "initialInput":state,
            "policy":{"policy":"policy.native-v2@1","default":"deny"},
            "root":{"kind":"seq","name":"root","state":state,"promotedStatePaths":[],"children":[
                {"kind":"verifier","name":"wait","worker":crate::native_v2_wait::WAIT_WORKER_REF,
                 "input":crate::native_v2_wait::input_schema().assert_value(),"output":{"kind":"null"},
                 "signals":crate::native_v2_wait::signal_labels().assert_value(),
                 "diagnostic":crate::native_v2_wait::diagnostic_schema().assert_value(),
                 "inputBindings":[{"target":["durationMs"],"value":{"source":"state","path":["durationMs"]}}],"writeBindings":[],"attempts":1},
                {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
            ]}
        })).assert_value();
        submission.initial_input = json!({"durationMs":7_200_000});
        let mut runtime = serde_json::to_value(&submission.runtime).assert_value();
        runtime["nodes"]["wait"] = json!({"kind":"request"});
        submission.runtime = serde_json::from_value(runtime).assert_value();
        let store = Arc::new(crate::native_v2_supervisor::checkpoints::fake_checkpoint_store(
            root.path(), storage.join("checkpoints"), workspace.clone(), Default::default(),
        ));
        let cleanup = Arc::new(PublicationStore::default());
        let controller = PortableRunController::start(
            bootstrap(run_id.clone(), submission, workspace, storage),
            |_| Ok::<_, std::io::Error>(PortableRuntime {
                checkpoints: Some(store), cleanup: cleanup.clone(),
                ..PortableRuntime::new(Arc::new(NeverDispatched))
            }),
        ).await.assert_value();
        while !matches!(controller.inner.status(RunStatusParams { run_id: run_id.clone() }).await.assert_value().status,
            RunStatus::Running { ref active_executions, .. } if active_executions.iter().any(|e| e.node.as_str() == "wait")) {
            tokio::task::yield_now().await;
        }
        controller.inner.suspend(RunSuspendParams { run_id: run_id.clone() }).await.assert_value();
        cleanup.disposing.notified().await;
        let first = controller.inner.steer(RunSteerParams {
            run_id: run_id.clone(), node: None, text: "during-local-cleanup".into(), mode: SteeringMode::Standing,
        }).await.assert_value();
        cleanup.disposal_release.notify_one();
        controller.wait_terminal().await.assert_value();
        drop(controller);
        let controller = PortableRunController::open_observer(paths.clone(), run_id.clone()).await.assert_value();
        let second = controller.inner.steer(RunSteerParams {
            run_id: run_id.clone(), node: None, text: "after-local-reopen".into(), mode: SteeringMode::Standing,
        }).await.assert_value();
        let ledger = SqliteRunLedger::open(paths.ledger()).assert_value();
        let directions = crate::v2_run_ledger::steering::all(&ledger, &run_id).await.assert_value();
        assert_eq!(directions.iter().map(|v| &v.id).collect::<Vec<_>>(), [&first.steer.id, &second.steer.id]);
        assert!(matches!(&directions[0].content, RunVectorContent::Steer { text, .. } if text == "during-local-cleanup"));
        assert!(matches!(&directions[1].content, RunVectorContent::Steer { text, .. } if text == "after-local-reopen"));
    }).await.assert_value();
}
