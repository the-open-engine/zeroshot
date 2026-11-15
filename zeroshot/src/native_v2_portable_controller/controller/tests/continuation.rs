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
        let mut events = vec![
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
            async |_| Ok::<_, std::io::Error>(PortableRuntime::new(Arc::new(NeverDispatched))),
            tokio::time::Instant::now() + bootstrap.preparation_timeout,
        )
        .await
        .assert_value();
        let runtime = prepared_runtime.runtime.assert_value();
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
