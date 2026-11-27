use super::*;
use crate::native_v2_files::{FileAccess, FilePin};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Default)]
pub(super) struct AnswerRollback {
    pub(super) entered: tokio::sync::Notify,
    pub(super) proceed: tokio::sync::Notify,
}

fn stage(
    files: &crate::native_v2_files::WorkspaceFiles,
    path: &std::path::Path,
    op: &str,
) -> (Value, FilePin) {
    let pin = FilePin {
        run_id: RunId::new("upload"),
        operation: IdempotencyKey::new(op).unwrap(),
    };
    let mut data = Vec::new();
    crate::native_v2_files::pack(path, &mut data).unwrap();
    files
        .host
        .stage_upload(
            &FileAccess {
                target: &files.target,
                identity: &files.identity,
            },
            &pin,
            &mut std::io::Cursor::new(data),
        )
        .unwrap();
    (
        json!({"kind":"staged","runId":pin.run_id,"operation":pin.operation}),
        pin,
    )
}
fn params(source: &RunId, output: Value) -> RunResumeParams {
    RunResumeParams {
        run_id: source.clone(),
        part_id: source.clone(),
        successor_run_id: other_run_id(),
        runtime: None,
        from: Some(openengine_cluster_protocol::RunResumeFrom::Answer {
            signals: serde_json::from_value(json!({"decision":"approved"})).unwrap(),
            output,
        }),
        connections: Default::default(),
        connection_resolver: None,
        github_token: None,
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,
    }
}

#[tokio::test]
async fn hosted_scalar_answers_preserve_upload_lookalikes_on_refusal_expiry_and_replay() {
    for mode in ["answer", "refused", "expired"] {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let files =
            crate::native_v2_cli::files::workspace_files(&root.path().join("state")).unwrap();
        let ledger = Arc::new(FakeRunLedger::new());
        let source = seed_with_output(
            &ledger,
            1,
            mode == "expired",
            json!({"kind":"record","fields":{
                "kind":{"required":true,"type":{"kind":"string"}},
                "runId":{"required":true,"type":{"kind":"string"}},
                "operation":{"required":true,"type":{"kind":"string"}}
            }}),
        )
        .await;
        let factory = Arc::new(RequestFactory {
            ledger: ledger.clone(),
            allocator: Arc::new(RequestAllocator {
                files: Some(files.clone()),
                ..Default::default()
            }),
        });
        let (_, endpoint, server) = direct_test_server(factory).await;
        let client = connect_client(&endpoint, None).await;
        let original = root.path().join("independent");
        std::fs::write(&original, "unrelated upload bytes").unwrap();
        let access = FileAccess {
            target: &files.target,
            identity: &files.identity,
        };
        for attempt in ["first", "replay"] {
            let (upload, pin) = stage(&files, &original, attempt);
            let mut answer = params(&source, upload);
            if mode == "refused" {
                let Some(openengine_cluster_protocol::RunResumeFrom::Answer { signals, .. }) =
                    &mut answer.from
                else {
                    unreachable!()
                };
                signals.clear();
            }
            let result = client.run_resume(answer).await;
            assert_eq!(
                result.is_ok(),
                mode == "answer" && attempt == "first",
                "{mode}: {result:?}"
            );
            assert!(
                files.host.read_pin(&access, &pin, &mut Vec::new()).is_ok(),
                "{mode}/{attempt} reclaimed ordinary scalar data as an upload"
            );
        }
        assert_eq!(
            ledger.list().await.unwrap().len(),
            if mode == "refused" { 1 } else { 2 }
        );
        server.abort();
    }
}

#[tokio::test]
async fn interrupted_answer_cannot_move_its_manifest_to_another_lineage() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let files = crate::native_v2_cli::files::workspace_files(&root.path().join("state")).unwrap();
    let ledger = Arc::new(FakeRunLedger::new());
    let schema =
        json!({"kind":"record","fields":{"data":{"required":true,"type":{"kind":"filepath"}}}});
    let source = seed_with_output(&ledger, 1, false, schema.clone()).await;
    let other = seed_with_output(&ledger, 2, false, schema.clone()).await;
    let original = root.path().join("data");
    std::fs::write(&original, "accepted bytes").unwrap();
    let (upload, _) = stage(&files, &original, "interrupted");
    let answer = json!({"data":upload});
    let first = params(&source, answer.clone());
    // Simulate controller loss at the cut after freezing, before creating the successor.
    let frozen = files
        .freeze_answer_inputs(crate::native_v2_files::inputs::InputAdmission {
            run_id: &first.successor_run_id,
            owner: &source,
            key: &IdempotencyKey::new(format!("resume-{}", first.successor_run_id.as_str()))
                .unwrap(),
            schema: &serde_json::from_value(schema).unwrap(),
            value: &answer,
            local_root: None,
        })
        .unwrap();
    assert!(ledger.get(&first.successor_run_id).await.unwrap().is_none());
    let controller = NativeV2PlatformController::new_with_delivery_policy(
        ledger.clone(),
        Arc::new(RequestAllocator {
            files: Some(files.clone()),
            ..Default::default()
        }),
        DeliveryPolicy::Optional,
    )
    .await
    .unwrap();
    let error = controller
        .resume(params(&other, answer.clone()))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("another successor or lineage"),
        "{error}"
    );
    assert!(ledger.get(&first.successor_run_id).await.unwrap().is_none());
    assert!(files.file_input_roots(&other).unwrap().is_empty());
    controller.resume(first).await.unwrap();
    std::fs::remove_file(original).unwrap();
    assert_eq!(
        files.materialize_file_inputs(&source, true).unwrap().len(),
        1
    );
    assert_eq!(
        std::fs::read(frozen.value["data"].as_str().unwrap()).unwrap(),
        b"accepted bytes"
    );
}

#[tokio::test]
async fn refused_answer_preserves_files_it_did_not_acquire() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let files = crate::native_v2_cli::files::workspace_files(&root.path().join("state")).unwrap();
    let ledger = Arc::new(FakeRunLedger::new());
    let source = seed_with_output(
        &ledger,
        1,
        false,
        json!({"kind":"record","fields":{"data":{"required":true,"type":{"kind":"filepath"}}}}),
    )
    .await;
    let controller = NativeV2PlatformController::new_with_delivery_policy(
        ledger,
        Arc::new(RequestAllocator {
            files: Some(files.clone()),
            refuse_claim: true.into(),
            ..Default::default()
        }),
        DeliveryPolicy::Optional,
    )
    .await
    .unwrap();
    let original = root.path().join("data");
    std::fs::write(&original, "accepted bytes").unwrap();
    let (upload, _) = stage(&files, &original, "prior-answer");
    let bad = params(&source, Value::Null);
    files
        .freeze_file_inputs(crate::native_v2_files::inputs::InputAdmission {
            run_id: &bad.successor_run_id,
            owner: &source,
            key: &IdempotencyKey::new(format!("resume-{}", bad.successor_run_id.as_str())).unwrap(),
            schema: &serde_json::from_value(json!({"kind":"record","fields":{"data":{"required":true,"type":{"kind":"filepath"}}}})).unwrap(),
            value: &json!({"data":upload}),
            local_root: None,
        })
        .unwrap();
    assert!(controller.resume(bad).await.is_err());
    // A retry that reuses the earlier manifest and then fails cannot roll that manifest back.
    assert!(
        controller
            .resume(params(&source, json!({"data":upload})))
            .await
            .is_err()
    );
    std::fs::remove_file(original).unwrap();
    let roots = files.materialize_file_inputs(&source, true).unwrap();
    assert_eq!(
        roots.len(),
        1,
        "a refusal without an input claim cannot release prior files"
    );
    let path = std::fs::read_dir(&roots[0])
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read(path).unwrap(), b"accepted bytes");
}

#[tokio::test]
async fn refused_answer_rollback_keeps_the_admission_lock() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let files = crate::native_v2_cli::files::workspace_files(&root.path().join("state")).unwrap();
    let ledger = Arc::new(FakeRunLedger::new());
    let source = seed_with_output(
        &ledger,
        1,
        false,
        json!({"kind":"record","fields":{"data":{"required":true,"type":{"kind":"filepath"}}}}),
    )
    .await;
    let rollback = Arc::new(AnswerRollback::default());
    let controller = NativeV2PlatformController::new_with_delivery_policy(
        ledger.clone(),
        Arc::new(RequestAllocator {
            files: Some(files.clone()),
            refuse_claim: true.into(),
            rollback: Some(rollback.clone()),
            ..Default::default()
        }),
        DeliveryPolicy::Optional,
    )
    .await
    .unwrap();
    let original = root.path().join("data");
    std::fs::write(&original, "accepted bytes").unwrap();
    let (upload, _) = stage(&files, &original, "refused");
    let failed = controller.resume(params(&source, json!({"data":upload})));
    tokio::pin!(failed);
    assert!(futures_util::poll!(&mut failed).is_pending());
    rollback.entered.notified().await;
    assert_eq!(files.file_input_roots(&source).unwrap().len(), 1);
    let (upload, _) = stage(&files, &original, "winner");
    let mut next = params(&source, json!({"data":upload}));
    next.successor_run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20d905");
    let winner_id = next.successor_run_id.clone();
    let winner = controller.resume(next);
    tokio::pin!(winner);
    assert!(futures_util::poll!(&mut winner).is_pending());
    assert!(
        ledger.get(&winner_id).await.unwrap().is_none(),
        "rollback still owns admission"
    );
    assert_eq!(files.file_input_roots(&source).unwrap().len(), 1);
    rollback.proceed.notify_one();
    assert!(failed.await.is_err());
    assert!(
        files.file_input_roots(&source).unwrap().is_empty(),
        "only refused files were released"
    );
    winner.await.unwrap();
    let roots = files.materialize_file_inputs(&source, true).unwrap();
    assert_eq!(roots.len(), 1);
    let path = std::fs::read_dir(&roots[0])
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read(path).unwrap(), b"accepted bytes");
}
#[tokio::test]
async fn hosted_file_answer_refusals_winner_replay_and_expiry_keep_one_owner() {
    for expired in [false, true] {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let files =
            crate::native_v2_cli::files::workspace_files(&root.path().join("state")).unwrap();
        let ledger = Arc::new(FakeRunLedger::new());
        let schema = json!({"kind":"record","fields":{
            "data":{"required":true,"type":{"kind":"filepath"}},
            "optional":{"required":false,"type":{"kind":"filepath"}}
        }});
        let source = seed_with_output(&ledger, 1, false, schema).await;
        let clock = Arc::new(AtomicU64::new(1000));
        let allocator = Arc::new(RequestAllocator {
            files: Some(files.clone()),
            clock: Some(clock.clone()),
            ..Default::default()
        });
        let factory = Arc::new(RequestFactory {
            ledger: ledger.clone(),
            allocator: allocator.clone(),
        });
        let (_, endpoint, server) = direct_test_server(factory).await;
        let client = connect_client(&endpoint, None).await;
        let original = root.path().join("data.csv");
        std::fs::write(&original, "private accepted bytes 0123456789").unwrap();
        let (missing, pin) = stage(&files, &original, "missing");
        assert!(
            client
                .run_resume(params(&source, json!({"optional":missing})))
                .await
                .is_err()
        );
        let access = FileAccess {
            target: &files.target,
            identity: &files.identity,
        };
        assert!(files.host.pin_exists(&access, &pin).is_err());
        let foreign = json!({"kind":"target","selection":{"address":{"scope":{"kind":"user"},"path":"not-authorized"},"member":""}});
        let mut unauthorized = params(&source, json!({"data":foreign}));
        unauthorized.successor_run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20d901");
        assert!(client.run_resume(unauthorized).await.is_err());
        assert_eq!(ledger.list().await.unwrap().len(), 1);
        let (upload, pin) = stage(&files, &original, "valid");
        let answer = params(&source, json!({"data":upload}));
        let mut expiry = answer.clone();
        expiry.successor_run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20d902");
        expiry.from = Some(openengine_cluster_protocol::RunResumeFrom::Expiry {});
        if expired {
            clock.store(
                openengine_cluster_protocol::MAX_SAFE_GENERATION,
                Ordering::SeqCst,
            );
        }
        let _results = tokio::join!(client.run_resume(answer.clone()), client.run_resume(expiry));
        assert!(
            client.run_resume(answer).await.is_err(),
            "lost response replay must not create a second successor"
        );
        assert_eq!(ledger.list().await.unwrap().len(), 2);
        assert_eq!(allocator.claims.lock().unwrap().len(), 1);
        assert!(
            files.host.pin_exists(&access, &pin).is_err(),
            "temporary upload reclaimed after winning or losing"
        );
        let (late, late_pin) = stage(&files, &original, "late");
        assert!(
            client
                .run_resume(params(&source, json!({"data":late})))
                .await
                .is_err()
        );
        assert!(
            files.host.pin_exists(&access, &late_pin).is_err(),
            "late upload must not leak"
        );
        std::fs::remove_file(&original).unwrap();
        let roots = files.materialize_file_inputs(&source, true).unwrap();
        assert_eq!(roots.len(), usize::from(!expired));
        if !expired {
            let path = std::fs::read_dir(&roots[0])
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "private accepted bytes 0123456789"
            );
            let successor = allocator.claims.lock().unwrap()[&source].clone();
            let stored = ledger.get(&successor).await.unwrap().unwrap();
            assert!(
                !serde_json::to_string(&stored)
                    .unwrap()
                    .contains("private accepted bytes")
            );
            // Allocation deliberately fails. The accepted answer is still durable and its pins
            // remain owned by the recovery lineage through another failed restart.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let status = client
                    .run_status(RunStatusParams::new(successor.clone()))
                    .await
                    .unwrap();
                if matches!(
                    status.status,
                    openengine_cluster_protocol::RunStatus::Finished { .. }
                ) {
                    break;
                }
                assert!(tokio::time::Instant::now() < deadline);
                tokio::task::yield_now().await;
            }
            let mut restart = params(&successor, Value::Null);
            restart.successor_run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20d903");
            restart.from = Some(openengine_cluster_protocol::RunResumeFrom::Restart {});
            client.run_resume(restart).await.unwrap();
            files.materialize_file_inputs(&source, true).unwrap();
            assert_eq!(
                std::fs::read_to_string(path).unwrap(),
                "private accepted bytes 0123456789"
            );
        }
        server.abort();
    }
}
