use super::*;
use super::super::local_contract_tests::{contract_submission, create_contract_run};
use crate::native_v2_candidate::test_support::{TestDirectory, full_graph, success_node};
use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeInstanceId};
use crate::v2_run_ledger::RunEvent;
use openengine_cluster_protocol::{RunSuspension, RunVector, RunVectorContent};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

fn suspension() -> RunSuspension {
    serde_json::from_value(json!({
        "reason":"request","node":"approval","createdAt":1000,
        "request":{"id":"question","text":"Approve?","signals":{},
            "output":{"kind":"record","fields":{"feedback":{"required":true,"type":{"kind":"string"}}}},
            "expiresAt":openengine_cluster_protocol::MAX_SAFE_GENERATION}
    })).assert_value()
}

fn resume(from: Option<RunResumeFrom>) -> openengine_cluster_protocol::RunResumeParams {
    openengine_cluster_protocol::RunResumeParams {
        run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000001"),
        successor_run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000002"),
        from,
        connections: Default::default(),
        connection_resolver: None,
        github_token: None,
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,
    }
}

#[test]
fn suspended_selection_refuses_rollback_and_preserves_answer_and_wait_rules() {
    let request = openengine_cluster_protocol::RunStatus::Suspended {
        suspension: suspension(),
        metadata: Default::default(),
    };
    let valid = RunResumeFrom::Answer {
        signals: Default::default(),
        output: json!({"feedback":"ok"}),
    };
    reject_suspended_selection(&request, Some(&valid), false).assert_value();
    assert!(
        reject_suspended_selection(&request, Some(&valid), true)
            .assert_error()
            .to_string()
            .contains("cannot discard")
    );
    for from in [
        None,
        Some(RunResumeFrom::Restart {}),
        Some(RunResumeFrom::Checkpoint {
            checkpoint_id: openengine_cluster_protocol::CheckpointId::new("checkpoint")
                .assert_value(),
        }),
    ] {
        assert!(
            reject_suspended_selection(&request, from.as_ref(), false)
                .assert_error()
                .to_string()
                .contains("zeroshot answer")
        );
    }
    for status in [
        openengine_cluster_protocol::RunStatus::Admitted {},
        serde_json::from_value(json!({"phase":"running","activeExecutions":[]})).assert_value(),
    ] {
        assert!(
            reject_suspended_selection(&status, Some(&valid), false)
                .assert_error()
                .to_string()
                .contains("requires a suspended run")
        );
    }
    for due in [
        None,
        Some(1),
        Some(openengine_cluster_protocol::MAX_SAFE_GENERATION),
    ] {
        let mut value = json!({"phase":"suspended","reason":if due.is_some() {"wait"} else {"command"},
            "node":"approval","createdAt":1000,"metadata":{}});
        if let Some(due) = due {
            value["wait"] = json!({"dueAt":due});
        }
        let status = serde_json::from_value(value).assert_value();
        assert!(
            reject_suspended_selection(&status, Some(&valid), false)
                .assert_error()
                .to_string()
                .contains("request suspension")
        );
        assert!(
            reject_suspended_selection(&status, Some(&RunResumeFrom::Restart {}), false)
                .assert_error()
                .to_string()
                .contains("only default resume")
        );
        let result = reject_suspended_selection(&status, None, false);
        if due == Some(openengine_cluster_protocol::MAX_SAFE_GENERATION) {
            assert!(result.assert_error().to_string().contains("wait is due at"));
        } else {
            result.assert_value();
        }
    }
}

async fn suspended_run(
    backend: &LocalCliBackend,
    expired: bool,
) -> (LocalRecoveryDocument, SqliteRunLedger) {
    let RunSuspension::Request {
        node,
        mut request,
        created_at,
    } = suspension()
    else {
        unreachable!()
    };
    if expired {
        request.expires_at =
            openengine_cluster_protocol::UnixTimestampMillis::new(2000).assert_value();
    }
    let mut submission = contract_submission("local-request-preflight");
    let input = crate::native_v2_request::input_schema().assert_value();
    let mut graph = serde_json::to_value(full_graph(vec![json!({
        "kind":"verifier", "name":node, "worker":crate::native_v2_request::ASK_WORKER_REF,
        "input":input, "output":request.output, "signals":request.signals,
        "diagnostic":crate::native_v2_wait::diagnostic_schema().assert_value(),
        "attempts":1, "inputBindings":[{"target":["text"],"value":{"source":"state","path":["text"]}}],
        "writeBindings":[]
    }), success_node()])).assert_value();
    graph["initialInput"] = json!(input);
    graph["root"]["state"] = json!(input);
    submission.graph = serde_json::from_value(graph).assert_value();
    submission.initial_input = json!({"text":"Approve?"});
    submission.runtime.nodes.insert(
        node.clone(),
        crate::native_v2_contract::NodeRuntimeBinding::Request {},
    );
    let run = resume(None).run_id;
    let ledger = create_contract_run(backend, &run, &submission).await;
    let reference = ExecutionRef {
        run_id: run.clone(),
        node: node.clone(),
        node_instance: NodeInstanceId::new(1).assert_value(),
        execution: ExecutionId::new(1).assert_value(),
    };
    ledger
        .append(
            &run,
            vec![
                RunEvent::RunStarted,
                RunEvent::NodeStarted {
                    reference: reference.clone(),
                    occurrence: crate::full_v1_reducer::StructuralOccurrence {
                        node: node.clone(),
                        map_indices: vec![],
                    },
                    attempt: openengine_cluster_protocol::PositiveInteger::new(1).assert_value(),
                    input: json!({"text":"Approve?"}),
                },
                RunEvent::RequestDeferred { reference },
                RunEvent::Vector {
                    vector: RunVector {
                        id: request.id.clone(),
                        run_id: run.clone(),
                        created_at,
                        content: RunVectorContent::Request {
                            node: node.clone(),
                            text: request.text.clone(),
                            signals: request.signals.clone(),
                            output: request.output.clone(),
                            expires_at: request.expires_at,
                        },
                    },
                },
                RunEvent::Suspended {
                    suspension: RunSuspension::Request {
                        node,
                        request,
                        created_at,
                    },
                },
            ],
        )
        .await
        .assert_value();
    let recovery = LocalRecoveryDocument {
        submission,
        workspace: backend.current_directory.clone(),
        delivery_receipt: None,
        local_branch: None,
        invoking_workspace: None,
        request_digest: None,
        workspace_revision: None,
        resumed_from: None,
        delivery_run_id: Some(run.clone()),
        successor_run_id: None,
        pending_restore: None,
    };
    backend
        .write_recovery_document(&run, &recovery)
        .assert_value();
    (recovery, ledger)
}

#[tokio::test]
async fn answer_preflight_errors_leave_no_successor_or_recovery_mutation() {
    let root = TestDirectory::new("local-answer-preflight");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let backend = LocalCliBackend::new(
        root.child("state"),
        root.child("missing-controller"),
        workspace,
        "git".into(),
    );
    let (recovery, ledger) = suspended_run(&backend, false).await;
    let run = resume(None).run_id;
    let before = ledger
        .get(&run)
        .await
        .assert_value()
        .assert_value()
        .snapshot;
    let recovery_bytes = std::fs::read(backend.recovery_path(&run).assert_value()).assert_value();
    for output in [
        json!({"feedback":false}),
        json!({"feedback":"x".repeat(crate::v2_run_ledger::MAX_EVENT_BYTES)}),
    ] {
        let params = resume(Some(RunResumeFrom::Answer {
            signals: Default::default(),
            output,
        }));
        let error = backend
            .start_local_successor(params.clone(), recovery.clone(), false)
            .await
            .assert_error();
        assert!(
            matches!(error, NativeV2CliError::Remote { code, message, .. } if code == "invalid_request" && message.contains("$.output"))
        );
        assert!(
            !backend
                .paths(&params.successor_run_id)
                .assert_value()
                .storage()
                .exists()
        );
        assert_eq!(
            ledger
                .get(&run)
                .await
                .assert_value()
                .assert_value()
                .snapshot,
            before
        );
        assert_eq!(
            std::fs::read(backend.recovery_path(&run).assert_value()).assert_value(),
            recovery_bytes
        );
    }
    let mut changed = recovery.clone();
    changed.submission.title =
        openengine_cluster_protocol::RunTitle::new("changed during preparation").assert_value();
    backend
        .write_recovery_document(&run, &changed)
        .assert_value();
    assert!(
        backend
            .require_unchanged_recovery(&run, &recovery)
            .assert_error()
            .to_string()
            .contains("record changed")
    );
    backend
        .require_unchanged_recovery(&run, &changed)
        .assert_value();
}

pub(in crate::native_v2_cli::local) async fn assert_local_inbox_reads_do_not_continue_requests() {
    use openengine_cluster_protocol::{
        InboxListParams, RequestState, RunVectorsParams, RunVectorParams, VectorId,
    };
    for expired in [false, true] {
        let root = TestDirectory::new("local-inbox-read");
        let workspace = root.child("workspace");
        std::fs::create_dir(&workspace).assert_value();
        let backend = LocalCliBackend::new(
            root.child("state"),
            root.child("missing-controller"),
            workspace,
            "git".into(),
        );
        let (_, ledger) = suspended_run(&backend, expired).await;
        let run = resume(None).run_id;
        let before = ledger
            .get(&run)
            .await
            .assert_value()
            .assert_value()
            .snapshot;
        let page = backend
            .inbox_list(None, InboxListParams::default())
            .await
            .assert_value();
        assert_eq!(page.requests.len(), 1);
        assert_eq!(page.requests[0].run_id, run);
        assert_eq!(
            page.requests[0].state,
            if expired {
                RequestState::Expired
            } else {
                RequestState::Pending
            }
        );
        assert!(page.next_after.is_none());
        let show = backend
            .inbox_show(
                None,
                RunStatusParams {
                    run_id: run.clone(),
                },
            )
            .await
            .assert_value();
        assert_eq!(show.state, page.requests[0].state);
        assert!(show.successor_run_id.is_none());
        for limit in [0, 101] {
            assert!(matches!(
                backend
                    .inbox_list(
                        None,
                        InboxListParams {
                            after: None,
                            limit: Some(limit)
                        }
                    )
                    .await,
                Err(NativeV2CliError::Usage(_))
            ));
        }
        assert!(
            backend
                .inbox_list(
                    None,
                    InboxListParams {
                        after: Some(run.clone()),
                        limit: Some(1)
                    }
                )
                .await
                .assert_value()
                .requests
                .is_empty()
        );
        let vectors = backend
            .run_vectors(
                None,
                RunVectorsParams {
                    run_id: run.clone(),
                    after: None,
                    limit: None,
                },
            )
            .await
            .assert_value();
        assert_eq!(vectors.vectors.len(), 1);
        let vector = backend
            .run_vector(
                None,
                RunVectorParams {
                    run_id: run.clone(),
                    id: VectorId::new("question").assert_value(),
                },
            )
            .await
            .assert_value();
        assert_eq!(vector.id.as_str(), "question");
        let missing = backend
            .run_vector(
                None,
                RunVectorParams {
                    run_id: run.clone(),
                    id: VectorId::new("absent").assert_value(),
                },
            )
            .await
            .assert_error();
        assert!(
            matches!(missing, NativeV2CliError::Remote {code, ..} if code == openengine_cluster_protocol::NOT_FOUND)
        );
        assert_eq!(
            ledger
                .get(&run)
                .await
                .assert_value()
                .assert_value()
                .snapshot,
            before
        );
        assert!(
            !backend
                .paths(&resume(None).successor_run_id)
                .assert_value()
                .storage()
                .exists()
        );
    }
}
