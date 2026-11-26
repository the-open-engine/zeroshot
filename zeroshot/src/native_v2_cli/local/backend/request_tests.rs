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
        runtime: None,
        part_id: RunId::new("0199f33f-3b44-7d21-9000-000000000001"),
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
    suspended_run_with_output(
        backend,
        expired,
        match suspension() {
            RunSuspension::Request { request, .. } => request.output,
            _ => unreachable!(),
        },
    )
    .await
}

async fn suspended_run_with_output(
    backend: &LocalCliBackend,
    expired: bool,
    output: openengine_cluster_protocol::PayloadType,
) -> (LocalRecoveryDocument, SqliteRunLedger) {
    let RunSuspension::Request {
        node,
        mut request,
        created_at,
    } = suspension()
    else {
        unreachable!()
    };
    request.output = output;
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
                            artifacts: Default::default(),
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
        workspace_checkpoints_discarded: false,
        pending_restore: None,
    };
    backend
        .write_recovery_document(&run, &recovery)
        .assert_value();
    (recovery, ledger)
}

#[tokio::test]
async fn force_stopped_suspension_explains_why_resume_is_unavailable() {
    let root = TestDirectory::new("stopped-resume");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let backend = LocalCliBackend::new(
        root.child("state"),
        "unused".into(),
        workspace,
        "git".into(),
    );
    let (_, ledger) = suspended_run(&backend, false).await;
    let run = resume(None).run_id;
    ledger
        .append(
            &run,
            vec![
                RunEvent::ForceStopRequested,
                RunEvent::Terminal {
                    result: openengine_cluster_protocol::TerminalResult::Failed {
                        reason: openengine_cluster_protocol::EnumLabel::new("force_stopped")
                            .assert_value(),
                        context: None,
                    },
                },
            ],
        )
        .await
        .assert_value();
    let before = backend
        .status_local(RunStatusParams::new(run.clone()))
        .await
        .assert_value();
    assert!(!before.workspace_recovery.recoverable);
    let error = backend
        .run_resume_status(None, RunStatusParams::new(run.clone()))
        .await
        .assert_error()
        .to_string();
    assert!(error.contains("suspension was force-stopped"), "{error}");
    assert_eq!(
        backend
            .status_local(RunStatusParams::new(run))
            .await
            .assert_value(),
        before
    );
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

#[tokio::test]
async fn refused_local_answer_preserves_files_owned_by_another_attempt() {
    let root = TestDirectory::new("local-refused-answer-files");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let backend = LocalCliBackend::new(
        root.child("state"),
        root.child("missing-controller"),
        workspace.clone(),
        "git".into(),
    );
    let (_, _ledger) = suspended_run(&backend, false).await;
    let params = resume(Some(RunResumeFrom::Answer {
        signals: Default::default(),
        output: json!({"feedback": false}),
    }));
    let original = workspace.join("data");
    std::fs::write(&original, "accepted bytes").assert_value();
    let files = crate::native_v2_cli::files::workspace_files(&backend.state_root).assert_value();
    let frozen = files
        .freeze_file_inputs(crate::native_v2_files::inputs::InputAdmission {
            run_id: &params.successor_run_id,
            owner: &params.run_id,
            key: &IdempotencyKey::new("earlier-answer").assert_value(),
            schema: &openengine_cluster_protocol::PayloadType::Filepath,
            value: &json!(original),
            local_root: Some(&workspace),
        })
        .assert_value();
    backend
        .run_resume(None, params.clone(), false)
        .await
        .assert_error();
    std::fs::write(&original, "user edit").assert_value();
    assert_eq!(
        files
            .materialize_file_inputs(&params.run_id, true)
            .assert_value()
            .len(),
        1
    );
    assert_eq!(
        std::fs::read(frozen.value.as_str().unwrap()).assert_value(),
        b"accepted bytes"
    );
    assert_eq!(std::fs::read(original).assert_value(), b"user edit");
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
            .inbox_show(None, RunStatusParams::new(run.clone()))
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
                    part_id: None,
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
                    part_id: None,
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
                    part_id: None,
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

#[tokio::test]
async fn file_answer_claims_freeze_replays_and_release_only_before_admission() {
    use super::super::local_contract_tests::prepared_local_run;
    use openengine_cluster_protocol::RequestResolution;
    for admitted in [false, true] {
        let root =
            tempfile::tempdir_in(std::env::temp_dir().canonicalize().assert_value()).assert_value();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).assert_value();
        let backend = LocalCliBackend::new(
            root.path().join("state"),
            "unused".into(),
            workspace.clone(),
            "git".into(),
        );
        let schema = serde_json::from_value(json!({"kind":"record","fields":{
            "file":{"required":true,"type":{"kind":"filepath"}}
        }}))
        .assert_value();
        let (recovery, _) = suspended_run_with_output(&backend, false, schema).await;
        let original = workspace.join("answer.csv");
        std::fs::write(&original, "accepted answer").assert_value();
        let params = resume(Some(RunResumeFrom::Answer {
            signals: Default::default(),
            output: json!({"file":original}),
        }));
        let mut prepared = prepared_local_run(
            params.successor_run_id.clone(),
            recovery.submission.clone(),
            workspace,
        );
        prepared.delivery_run_id = params.run_id.clone();
        let mut resolution = backend.resolve_local_request(&params).await.assert_value();
        let claim = backend
            .accept_request_files(&params, &prepared, &mut resolution)
            .await
            .assert_value()
            .assert_value();
        let Some(RequestResolution::Answer { answer }) = &resolution else {
            panic!("expected answer")
        };
        let accepted = PathBuf::from(answer.output["file"].as_str().assert_value());
        assert_ne!(accepted, original);
        std::fs::write(&original, "edited original").assert_value();
        let mut replay = backend.resolve_local_request(&params).await.assert_value();
        assert!(
            backend
                .accept_request_files(&params, &prepared, &mut replay)
                .await
                .assert_value()
                .is_none()
        );
        assert_eq!(replay, resolution);
        let files =
            crate::native_v2_cli::files::workspace_files(&backend.state_root).assert_value();
        assert_eq!(
            files
                .materialize_file_inputs(&params.run_id, true)
                .assert_value()
                .len(),
            1
        );
        assert_eq!(std::fs::read(&accepted).assert_value(), b"accepted answer");
        if admitted {
            create_contract_run(&backend, &params.successor_run_id, &recovery.submission).await;
        }
        backend
            .release_refused_input_claim(&params.successor_run_id, &claim)
            .await
            .assert_value();
        assert_eq!(
            files.file_input_roots(&params.run_id).assert_value().len(),
            usize::from(admitted)
        );
        assert_eq!(accepted.exists(), admitted);
        assert_eq!(std::fs::read(&original).assert_value(), b"edited original");
    }
}

#[tokio::test]
async fn file_answer_validation_rolls_back_its_own_pin_without_dispatching() {
    use super::super::local_contract_tests::prepared_local_run;
    use openengine_cluster_protocol::{RequestAnswer, RequestResolution};
    let root =
        tempfile::tempdir_in(std::env::temp_dir().canonicalize().assert_value()).assert_value();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let backend = LocalCliBackend::new(
        root.path().join("state"),
        "unused".into(),
        workspace.clone(),
        "git".into(),
    );
    let schema = serde_json::from_value(json!({"kind":"record","fields":{
        "file":{"required":true,"type":{"kind":"filepath"}},
        "feedback":{"required":true,"type":{"kind":"string"}}
    }}))
    .assert_value();
    let (recovery, ledger) = suspended_run_with_output(&backend, false, schema).await;
    let before = ledger
        .get(&resume(None).run_id)
        .await
        .assert_value()
        .assert_value();
    let params = resume(None);
    let mut prepared = prepared_local_run(
        params.successor_run_id.clone(),
        recovery.submission,
        workspace.clone(),
    );
    prepared.delivery_run_id = params.run_id.clone();
    let original = workspace.join("answer.csv");
    std::fs::write(&original, "keep original").assert_value();
    // Exercise the adapter's final validation after it has acquired a file claim.
    let mut resolution = Some(RequestResolution::Answer {
        answer: RequestAnswer {
            signals: Default::default(),
            output: json!({"file":original,"feedback":"x".repeat(crate::v2_run_ledger::MAX_EVENT_BYTES)}),
        },
    });
    let error = backend
        .accept_request_files(&params, &prepared, &mut resolution)
        .await
        .assert_error();
    assert!(error.to_string().contains("exceeds"), "{error}");
    let files = crate::native_v2_cli::files::workspace_files(&backend.state_root).assert_value();
    assert!(
        files
            .file_input_roots(&params.run_id)
            .assert_value()
            .is_empty()
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
            .get(&params.run_id)
            .await
            .assert_value()
            .assert_value(),
        before
    );
    assert_eq!(std::fs::read(original).assert_value(), b"keep original");
    assert!(
        backend
            .resolve_local_request(&params)
            .await
            .assert_value()
            .is_none()
    );
}

#[tokio::test]
async fn file_answer_refusals_preserve_history_and_create_no_successor() {
    use super::super::local_contract_tests::prepared_local_run;
    use openengine_cluster_protocol::{RequestAnswer, RequestResolution};
    for state in ["missing", "admitted", "upload-unavailable"] {
        let root =
            tempfile::tempdir_in(std::env::temp_dir().canonicalize().assert_value()).assert_value();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).assert_value();
        let original = workspace.join("original.csv");
        std::fs::write(&original, "preserve original").assert_value();
        let backend = LocalCliBackend::new(
            root.path().join("state"),
            "unused".into(),
            workspace.clone(),
            "git".into(),
        );
        let params = resume(Some(RunResumeFrom::Answer {
            signals: Default::default(),
            output: json!({"file":{"kind":"staged","runId":"missing-upload","operation":"answer"}}),
        }));
        let (submission, ledger) = if state == "upload-unavailable" {
            let schema = serde_json::from_value(json!({"kind":"record","fields":{
                "file":{"required":true,"type":{"kind":"filepath"}}
            }}))
            .assert_value();
            let (recovery, ledger) = suspended_run_with_output(&backend, false, schema).await;
            (recovery.submission, ledger)
        } else {
            let submission = contract_submission("file-answer-refusal");
            let ledger = if state == "missing" {
                let storage = backend.create_run_storage(&params.run_id).assert_value();
                SqliteRunLedger::open(storage.join("runs.sqlite3")).assert_value()
            } else {
                create_contract_run(&backend, &params.run_id, &submission).await
            };
            (submission, ledger)
        };
        let before = ledger.get(&params.run_id).await.assert_value();
        let mut prepared =
            prepared_local_run(params.successor_run_id.clone(), submission, workspace);
        prepared.delivery_run_id = params.run_id.clone();
        let resolved = backend.resolve_local_request(&params).await;
        if state == "upload-unavailable" {
            assert!(resolved.assert_value().is_some());
        } else {
            let error = resolved.assert_error().to_string();
            assert!(error.contains("suspended"), "{state}: {error}");
        }
        let RunResumeFrom::Answer { signals, output } = params.from.clone().assert_value() else {
            unreachable!()
        };
        let mut resolution = Some(RequestResolution::Answer {
            answer: RequestAnswer { signals, output },
        });
        let error = backend
            .accept_request_files(&params, &prepared, &mut resolution)
            .await
            .assert_error()
            .to_string();
        let expected = match state {
            "missing" => "request is unavailable",
            "admitted" => "run has no request",
            _ => "$.output:",
        };
        assert!(error.contains(expected), "{state}: {error}");
        backend
            .release_answer_uploads(&params.run_id, &json!({}))
            .await
            .assert_value();
        assert_eq!(ledger.get(&params.run_id).await.assert_value(), before);
        assert!(
            !backend
                .paths(&params.successor_run_id)
                .assert_value()
                .storage()
                .exists()
        );
        if let Some(files) =
            crate::native_v2_cli::files::input_files_if_present(&backend.state_root).assert_value()
        {
            assert!(
                files
                    .file_input_roots(&params.run_id)
                    .assert_value()
                    .is_empty()
            );
        }
        assert_eq!(std::fs::read(original).assert_value(), b"preserve original");
    }
}
