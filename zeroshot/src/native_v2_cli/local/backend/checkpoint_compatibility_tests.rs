use super::*;
use super::super::local_contract_tests::contract_submission;
use crate::native_v2_candidate::test_support::TestDirectory;
use openengine_cluster_protocol::CheckpointId;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

fn params(from: Option<RunResumeFrom>) -> openengine_cluster_protocol::RunResumeParams {
    openengine_cluster_protocol::RunResumeParams {
        runtime: None,
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,

        run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000001"),
        successor_run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000002"),
        from,
        connections: Default::default(),
        connection_resolver: None,
        github_token: None,
    }
}

fn selected_checkpoint() -> Option<RunResumeFrom> {
    Some(RunResumeFrom::Checkpoint {
        checkpoint_id: CheckpointId::new("entry-1").assert_value(),
    })
}

fn recovery(root: &TestDirectory, checkpoints: bool) -> LocalRecoveryDocument {
    let mut submission = contract_submission("checkpoint-compatibility");
    submission.runtime.workspace_checkpoints = checkpoints;
    LocalRecoveryDocument {
        delivery_receipt: None,
        local_branch: None,
        invoking_workspace: None,
        request_digest: None,
        workspace_revision: None,
        submission,
        workspace: root.child("workspace"),
        resumed_from: None,
        delivery_run_id: None,
        successor_run_id: None,
        workspace_checkpoints_discarded: false,
        pending_restore: None,
    }
}

#[tokio::test]
async fn resume_runtime_requirements_include_shared_delivery_credentials() {
    use crate::native_v2_candidate::test_support::{full_graph, git_delivery_node, success_node};
    use crate::native_v2_delivery::DeliveryMode;
    use openengine_cluster_protocol::RunResumeRuntime;
    use serde_json::json;

    for mode in [DeliveryMode::PullRequest, DeliveryMode::Merge] {
        for uniform in [false, true] {
            let root = TestDirectory::new("resume-credentials");
            let backend = LocalCliBackend::new(
                root.child("state"),
                PathBuf::from("zeroshot"),
                root.path().to_owned(),
                PathBuf::from("git"),
            );
            let mut recovery = recovery(&root, false);
            std::fs::create_dir(&recovery.workspace).assert_value();
            let mut delivery = git_delivery_node();
            delivery["worker"] = json!(mode.worker_ref());
            delivery["output"] = serde_json::to_value(
                crate::native_v2_delivery::contract::delivery_result_schema(mode).assert_value(),
            )
            .assert_value();
            delivery["signals"] = json!({
                "delivery": crate::native_v2_delivery::delivery_signal_labels(mode).assert_value()
            });
            recovery.submission.graph = full_graph(vec![
                json!({
                    "kind":"step", "name":"worker", "worker":"agent.work@1",
                    "instructions":"Continue the work.",
                    "input":{"kind":"null"}, "output":{"kind":"null"},
                    "inputBindings":[], "writeBindings":[], "timeoutMs":1000, "attempts":1
                }),
                delivery,
                success_node(),
            ]);
            recovery.submission.source = Some(
                serde_json::from_value(json!({
                    "repository":"team/project", "branch":"main", "revision":"a".repeat(40),
                    "remote":{"url":"https://gitlab.com/team/project.git", "forge":{
                        "provider":"source.gitlab", "repository":"team/project",
                        "baseUrl":"https://gitlab.com"
                    }}
                }))
                .assert_value(),
            );
            let replacement: RunResumeRuntime = serde_json::from_value(json!({
                "kind":"uniform", "runtime":{
                    "harness":"codex", "provider":"openai", "model":"corrected-model",
                    "workspaceCheckpoints":false,
                    "connections":{"gitlab":["OPENAI_API_KEY"]}
                }
            }))
            .assert_value();
            recovery.submission.runtime = crate::native_v2_runtime_config::resolve(
                replacement.clone(),
                &recovery.submission.graph,
            );
            let replacement = if uniform {
                replacement
            } else {
                RunResumeRuntime::Exact {
                    runtime: recovery.submission.runtime.clone(),
                }
            };
            let mut request = params(None);
            request.runtime = Some(replacement.clone());
            backend.create_run_storage(&request.run_id).assert_value();
            backend
                .write_recovery_document(&request.run_id, &recovery)
                .assert_value();
            let recovery_path = backend.recovery_path(&request.run_id).assert_value();
            let original = std::fs::read(&recovery_path).assert_value();
            let requirements = backend
                .replacement_resume_requirements(
                    &RunSelector {
                        target: None,
                        run_id: request.run_id.clone(),
                    },
                    &replacement,
                    false,
                )
                .await
                .assert_value();
            assert_eq!(
                serde_json::to_value(&requirements).assert_value(),
                json!({"gitlab":["GITLAB_TOKEN", "OPENAI_API_KEY"]})
            );
            // Select exactly the advertised fields, as the CLI does from the caller's environment.
            request.connections = requirements
                .into_iter()
                .map(|(key, fields)| {
                    let values = fields
                        .into_iter()
                        .map(|field| (field, "fresh-fixture-value".to_owned()))
                        .collect();
                    (
                        key,
                        openengine_cluster_protocol::StaticConnectionValues::new(values)
                            .assert_value(),
                    )
                })
                .collect();
            let prepared = backend
                .prepare_successor(&request, &recovery)
                .await
                .assert_value();
            assert_eq!(prepared.environment.bootstrap_values(), request.connections);
            assert_eq!(prepared.submission.graph, recovery.submission.graph);
            assert_eq!(prepared.submission.source, recovery.submission.source);
            assert_eq!(std::fs::read(recovery_path).assert_value(), original);
            assert!(
                !backend
                    .paths(&request.successor_run_id)
                    .assert_value()
                    .storage()
                    .exists()
            );
        }
    }
}

#[tokio::test]
async fn resume_runtime_bounds_effective_delivery_defaults_and_preserves_native_token_access() {
    use crate::native_v2_candidate::test_support::{full_graph, git_delivery_node, success_node};
    use openengine_cluster_protocol::{DeclaredConnections, NodeName, NodeRuntimeBinding};
    use serde_json::json;

    let root = TestDirectory::new("resume-size");
    let mut recovery = recovery(&root, true);
    let mut graph =
        serde_json::to_value(full_graph(vec![git_delivery_node(), success_node()])).assert_value();
    let state =
        json!({"kind":"record","fields":{"padding":{"type":{"kind":"string"},"required":true}}});
    graph["initialInput"] = state.clone();
    graph["root"]["state"] = state;
    recovery.submission.graph = serde_json::from_value(graph).assert_value();
    recovery.submission.initial_input = json!({"padding":""});
    recovery.submission.runtime.nodes.insert(
        NodeName::new("deliver").assert_value(),
        NodeRuntimeBinding::GitDelivery {
            connections: DeclaredConnections::empty(),
            pull_request_feedback: Default::default(),
        },
    );
    let admitted = crate::native_v2_admission::NativeV2Admission
        .admit(recovery.submission.clone())
        .await
        .unwrap();
    let overhead = serde_json::to_vec(&admitted).assert_value().len();
    recovery.submission.initial_input =
        json!({"padding":"x".repeat(crate::v2_run_ledger::MAX_ADMITTED_RUN_BYTES - overhead)});
    let config = openengine_cluster_protocol::RunResumeRuntime::Exact {
        runtime: recovery.submission.runtime.clone(),
    };
    let error = admit_resume_runtime(&recovery, &config, &BTreeMap::new(), false)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("exceeding the 2 MiB"), "{error}");
    // Native GitHub authority deliberately keeps this binding empty. Its unchanged definition
    // fits exactly; adding an unnecessary connection would incorrectly reject that valid run.
    for (environment, github_token) in [
        (BTreeMap::new(), true),
        (
            BTreeMap::from([(
                crate::native_v2_delivery::GITHUB_TOKEN_FILE_ENV.to_owned(),
                "owned-token-file".to_owned(),
            )]),
            false,
        ),
    ] {
        let runtime = admit_resume_runtime(&recovery, &config, &environment, github_token)
            .await
            .unwrap();
        assert!(
            runtime.nodes()[&NodeName::new("deliver").assert_value()]
                .declared_connections()
                .is_empty()
        );
        if github_token {
            let backend = backend(&root);
            let run_id = params(None).run_id;
            backend.create_run_storage(&run_id).assert_value();
            backend
                .write_recovery_document(&run_id, &recovery)
                .assert_value();
            assert!(
                backend
                    .replacement_resume_requirements(
                        &RunSelector {
                            target: None,
                            run_id
                        },
                        &config,
                        true,
                    )
                    .await
                    .assert_value()
                    .is_empty()
            );
        }
    }
}

#[tokio::test]
async fn dead_controller_resume_retains_acknowledged_receipts_without_importing_graph_history() {
    use crate::native_v2_contract::NodeRuntimeBinding;
    use crate::native_v2_candidate::test_support::{full_graph, git_delivery_node, success_node};
    use crate::native_v2_supervisor::checkpoints::{DeliveryCheckpointStore, RunCheckpointStore};
    use crate::native_v2_runner::test_support::{delivery_checkpoint, node_started};
    use crate::v2_run_ledger::RunEvent;
    use openengine_cluster_protocol::{DeclaredConnections, NodeName};
    use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeInstanceId};
    use serde_json::{Value, json};
    for enabled in [false, true] {
        let root = TestDirectory::new("dead-delivery");
        let backend = backend(&root);
        let mut recovery = recovery(&root, enabled);
        recovery.submission.graph = full_graph(vec![git_delivery_node(), success_node()]);
        recovery.submission.runtime.nodes.insert(
            NodeName::new("deliver").assert_value(),
            NodeRuntimeBinding::GitDelivery {
                connections: DeclaredConnections::empty(),
                pull_request_feedback: Default::default(),
            },
        );
        let run_id = params(None).run_id;
        let ledger = super::super::local_contract_tests::create_contract_run(
            &backend,
            &run_id,
            &recovery.submission,
        )
        .await;
        let repository = backend.checkpoint_repository(&run_id).assert_value();
        let store = DeliveryCheckpointStore::new(repository.clone());
        let old = delivery_checkpoint(json!({"receipts":[]}), 1);
        store.retain_delivery_state(&old).await.assert_value();
        let latest = delivery_checkpoint(json!({"owned":"resolved revision"}), 2);
        let reference = ExecutionRef {
            run_id: run_id.clone(),
            node: NodeName::new("deliver").assert_value(),
            node_instance: NodeInstanceId::new(1).assert_value(),
            execution: ExecutionId::new(1).assert_value(),
        };
        ledger
            .append(
                &run_id,
                vec![
                    RunEvent::RunStarted,
                    node_started(&reference, Value::Null),
                    RunEvent::DeliveryState {
                        execution: reference.execution,
                        state: latest.clone(),
                    },
                ],
            )
            .await
            .assert_value();
        drop(ledger);
        assert_eq!(store.delivery_state().await.assert_value(), Some(old));
        // Opening an observer reconciles a hard process exit without constructing a runtime.
        let observer = PortableRunController::open_observer(
            backend.paths(&run_id).assert_value(),
            run_id.clone(),
        )
        .await
        .assert_value();
        drop(observer);
        for _ in 0..2 {
            backend
                .retain_predecessor_delivery(&run_id, &recovery)
                .await
                .assert_value();
        }
        assert_eq!(
            store.delivery_state().await.assert_value(),
            Some(latest.clone())
        );
        assert_eq!(std::fs::read_dir(repository).assert_value().count(), 1);
        assert!(
            !backend
                .paths(&run_id)
                .assert_value()
                .storage()
                .join("checkpoints/index.json")
                .exists()
        );
        let predecessor = SqliteRunLedger::open(backend.paths(&run_id).assert_value().ledger())
            .assert_value()
            .get(&run_id)
            .await
            .assert_value()
            .assert_value();
        assert!(predecessor.snapshot.execution_seed.is_empty());
        assert!(
            matches!(predecessor.snapshot.terminal, Some(TerminalResult::Failed { reason, .. })
            if reason.as_str() == "runtime_lost")
        );
        // A same-generation disagreement blocks successor preparation and preserves retained bytes.
        store.discard().await.assert_value();
        let conflict = delivery_checkpoint(json!({"conflicting":"receipt"}), 2);
        store.retain_delivery_state(&conflict).await.assert_value();
        assert!(
            backend
                .retain_predecessor_delivery(&run_id, &recovery)
                .await
                .is_err()
        );
        assert_eq!(store.delivery_state().await.assert_value(), Some(conflict));
    }
}

/// Publishes a latest snapshot of `workspace` for the predecessor of [`params`].
async fn publish_latest(root: &TestDirectory, backend: &LocalCliBackend, workspace: &Path) {
    use crate::native_v2_supervisor::checkpoints::{RunCheckpointStore, fake_checkpoint_store};
    let directory = checkpoint_directory(backend);
    fake_checkpoint_store(
        root.path(),
        directory,
        workspace.to_owned(),
        Default::default(),
    )
    .finish(&[])
    .await
    .assert_value();
}

fn checkpoint_directory(backend: &LocalCliBackend) -> PathBuf {
    backend
        .paths(&params(None).run_id)
        .assert_value()
        .storage()
        .join("checkpoints")
}

#[tokio::test]
async fn plain_resume_keeps_the_checkout_even_when_a_latest_snapshot_exists() {
    let root = TestDirectory::new("lpk");
    let backend = backend(&root);
    let recovery = recovery(&root, true);
    std::fs::create_dir(&recovery.workspace).assert_value();
    std::fs::write(recovery.workspace.join("source.txt"), "snapshot").assert_value();
    publish_latest(&root, &backend, &recovery.workspace).await;
    assert!(checkpoint_directory(&backend).join("latest.json").is_file());

    for from in [None, Some(RunResumeFrom::Restart {})] {
        assert!(
            backend
                .checkpoint_selection(&params(from.clone()), &recovery, false)
                .assert_value()
                .is_none()
        );
        // Consent without a selected checkpoint has nothing to replace.
        let error = backend
            .checkpoint_selection(&params(from), &recovery, true)
            .assert_error();
        assert!(
            matches!(&error, NativeV2CliError::Usage(message)
                if message.contains("--discard-workspace-changes requires --from-checkpoint")),
            "{error}"
        );
    }
}

#[test]
fn selected_checkpoint_requires_consent_before_it_is_pinned() {
    let root = TestDirectory::new("lsc");
    let backend = backend(&root);
    let recovery = recovery(&root, true);

    let error = backend
        .checkpoint_selection(&params(selected_checkpoint()), &recovery, false)
        .assert_error();
    let NativeV2CliError::Usage(message) = &error else {
        panic!("a rollback without consent must be a usage error: {error}");
    };
    for expected in [
        "restoring checkpoint entry-1 discards this workspace's",
        "untracked, and ignored files",
        "and its index",
        "attached HEAD",
        "branches and commits are kept",
        "zeroshot resume 0199f33f-3b44-7d21-9000-000000000001 --from-checkpoint entry-1 \
         --discard-workspace-changes",
    ] {
        assert!(message.contains(expected), "{message}");
    }
    // A consented checkpoint is pinned before launch, so an unavailable one fails without a
    // controller instead of falling back to the retained workspace.
    let error = backend
        .checkpoint_selection(&params(selected_checkpoint()), &recovery, true)
        .assert_error();
    assert!(
        error
            .to_string()
            .contains("workspace checkpoint is unavailable"),
        "{error}"
    );
}

#[test]
fn interrupted_restore_blocks_a_resume_that_would_not_complete_it() {
    let root = TestDirectory::new("lir");
    let backend = backend(&root);
    let mut recovery = recovery(&root, true);
    let command = "zeroshot resume 0199f33f-3b44-7d21-9000-000000000001";
    for (selection, hint) in [
        (
            CheckpointRestoreSelection::Latest,
            format!("`{command} --discard-workspace-changes`"),
        ),
        (
            CheckpointRestoreSelection::Checkpoint {
                checkpoint_id: CheckpointId::new("entry-1").assert_value(),
            },
            format!("`{command} --from-checkpoint entry-1 --discard-workspace-changes`"),
        ),
    ] {
        let legacy = selection == CheckpointRestoreSelection::Latest;
        recovery.pending_restore = Some(PendingRestore {
            selection,
            snapshot: serde_json::from_value(serde_json::json!(uuid::Uuid::now_v7().to_string()))
                .assert_value(),
            attempt: RunId::new("0199f33f-3b44-7d21-9000-000000000003"),
        });
        for from in [None, Some(RunResumeFrom::Restart {})] {
            // Consent completes only an interrupted latest restore; a checkpoint names its own.
            for discard in [false, !legacy] {
                let error = backend
                    .checkpoint_selection(&params(from.clone()), &recovery, discard)
                    .assert_error()
                    .to_string();
                assert!(
                    error.contains("partially restored") && error.contains(&hint),
                    "{error}"
                );
            }
        }
    }
}

#[tokio::test]
async fn interrupted_latest_restore_completes_only_with_consent_and_its_pinned_snapshot() {
    use crate::native_v2_supervisor::checkpoints::pin;

    let root = TestDirectory::new("restore-pin");
    let backend = backend(&root);
    let mut recovery = recovery(&root, true);
    let workspace = recovery.workspace.clone();
    std::fs::create_dir(&workspace).assert_value();
    let source = workspace.join("source.txt");
    std::fs::write(&source, "original").assert_value();
    publish_latest(&root, &backend, &workspace).await;
    let request = params(None);
    // Earlier releases recorded this pending restore for a plain resume.
    let snapshot = pin(&CheckpointRestore {
        directory: checkpoint_directory(&backend),
        selection: CheckpointRestoreSelection::Latest,
    })
    .assert_value();
    recovery.pending_restore = Some(PendingRestore {
        selection: CheckpointRestoreSelection::Latest,
        snapshot: snapshot.clone(),
        attempt: RunId::new("0199f33f-3b44-7d21-9000-000000000003"),
    });

    let error = backend
        .checkpoint_selection(&request, &recovery, false)
        .assert_error();
    assert!(error.to_string().contains("partially restored"), "{error}");
    let (restore, pending) = backend
        .checkpoint_selection(&request, &recovery, true)
        .assert_value()
        .assert_value();
    assert_eq!(restore.selection, CheckpointRestoreSelection::Latest);
    assert_eq!(pending.selection, CheckpointRestoreSelection::Latest);
    assert_eq!(pending.snapshot, snapshot);
    assert_eq!(pending.attempt, request.successor_run_id);

    std::fs::write(&source, "newer workspace").assert_value();
    publish_latest(&root, &backend, &workspace).await;
    let error = backend
        .checkpoint_selection(&request, &recovery, true)
        .assert_error();
    assert!(
        error
            .to_string()
            .contains("no longer resolves to its pinned snapshot"),
        "{error}"
    );
    assert_eq!(
        recovery.pending_restore.as_ref().assert_value().snapshot,
        snapshot
    );
    assert_eq!(
        std::fs::read_to_string(source).assert_value(),
        "newer workspace"
    );
    assert!(
        !backend
            .paths(&request.successor_run_id)
            .assert_value()
            .storage()
            .exists()
    );
}

#[test]
fn disabled_workspace_checkpoints_allow_restart_but_reject_snapshot_restore() {
    let root = TestDirectory::new("checkpoint-policy");
    let backend = backend(&root);
    let recovery = recovery(&root, false);
    for from in [None, Some(RunResumeFrom::Restart {})] {
        assert!(
            backend
                .checkpoint_selection(&params(from), &recovery, false)
                .assert_value()
                .is_none()
        );
    }
    for discard in [false, true] {
        let error = backend
            .checkpoint_selection(&params(selected_checkpoint()), &recovery, discard)
            .unwrap_err();
        assert!(error.to_string().contains("workspaceCheckpoints is false"));
    }
}

fn backend(root: &TestDirectory) -> LocalCliBackend {
    LocalCliBackend::new(
        root.path().to_owned(),
        PathBuf::from("zeroshot"),
        root.path().to_owned(),
        PathBuf::from("git"),
    )
}
