use super::*;
use crate::native_v2_cli::local::local_contract_tests::{contract_submission, create_contract_run};
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_cli::{
    execute_native_v2_cli, NativeV2CliCommand, NeverDetach, RunSelector, SteerText,
};
use crate::v2_run_ledger::RunEvent;
use openengine_cluster_protocol::{
    RunSteerParams, RunSteerResult, RunStatus, RunSuspension, UnixTimestampMillis,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

fn backend(root: &TestDirectory) -> LocalCliBackend {
    LocalCliBackend::new(
        root.child("state"),
        root.child("must-not-launch"),
        root.path().into(),
        "git".into(),
    )
}

fn params() -> RunSteerParams {
    RunSteerParams {
        run_id: RunId::new("0199f33f-3b44-7d21-9000-000000000071"),
        text: "  Keep\t方向\r\nexactly.  ".into(),
        mode: openengine_cluster_protocol::SteeringMode::Standing,
    }
}

#[tokio::test]
async fn steer_local_list_omits_direction_while_status_keeps_bounded_summaries() {
    let root = TestDirectory::new("steer-local-list");
    let backend = backend(&root);
    let params = params();
    let ledger = create_contract_run(
        &backend,
        &params.run_id,
        &contract_submission("steer-local-list"),
    )
    .await;
    ledger
        .append(
            &params.run_id,
            vec![
                RunEvent::RunStarted,
                RunEvent::Suspended {
                    suspension: RunSuspension::Command {
                        node: openengine_cluster_protocol::NodeName::new("next").assert_value(),
                        created_at: UnixTimestampMillis::MIN,
                    },
                },
            ],
        )
        .await
        .assert_value();
    for _ in 0..65 {
        backend.run_steer(None, params.clone()).await.assert_value();
    }
    for finished in [false, true] {
        if finished {
            ledger
                .append(
                    &params.run_id,
                    vec![
                        RunEvent::ForceStopRequested,
                        RunEvent::Terminal {
                            result: TerminalResult::Succeeded {
                                output: serde_json::Value::Null,
                            },
                        },
                    ],
                )
                .await
                .assert_value();
        }
        let status = backend
            .run_status(
                None,
                RunStatusParams {
                    run_id: params.run_id.clone(),
                },
            )
            .await
            .assert_value();
        assert_eq!(status.steering.steers.len(), 64);
        assert_eq!(status.steering.omitted_steers, 1);
        let listed = serde_json::to_value(
            backend
                .run_list(None, RunListParams::default())
                .await
                .assert_value(),
        )
        .assert_value();
        let mut expected = serde_json::to_value(status).assert_value();
        expected.as_object_mut().assert_value().remove("steers");
        expected
            .as_object_mut()
            .assert_value()
            .remove("omittedSteers");
        assert_eq!(listed, serde_json::json!({"runs": [expected]}));
    }
}

#[tokio::test]
async fn steer_suspended_local_command_records_full_text_without_resuming_and_refuses_successor() {
    let root = TestDirectory::new("steer-local-record");
    let backend = backend(&root);
    let params = params();
    let submission = contract_submission("steer-local-record");
    let ledger = create_contract_run(&backend, &params.run_id, &submission).await;
    ledger
        .append(
            &params.run_id,
            vec![
                RunEvent::RunStarted,
                RunEvent::Suspended {
                    suspension: RunSuspension::Command {
                        node: openengine_cluster_protocol::NodeName::new("next").assert_value(),
                        created_at: UnixTimestampMillis::MIN,
                    },
                },
            ],
        )
        .await
        .assert_value();
    let path = root.child("direction.txt");
    std::fs::write(&path, &params.text).assert_value();
    let mut output = Vec::new();
    execute_native_v2_cli(
        NativeV2CliCommand::Steer {
            run: RunSelector {
                target: None,
                run_id: params.run_id.clone(),
            },
            text: SteerText::File(path),
            networks: vec![],
            mode: openengine_cluster_protocol::SteeringMode::Standing,
        },
        &backend,
        &mut NeverDetach,
        &mut output,
    )
    .await
    .assert_value();
    let recorded: RunSteerResult = serde_json::from_slice(&output).assert_value();
    assert_eq!(recorded.run_id, params.run_id);
    assert_eq!(recorded.steer.excerpt, params.text);
    assert_eq!(recorded.steer.text_bytes, params.text.len());
    assert_eq!(recorded.steer.received_executions, 0);
    let status = backend
        .run_status(
            None,
            RunStatusParams {
                run_id: params.run_id.clone(),
            },
        )
        .await
        .assert_value();
    assert!(matches!(
        status.status,
        openengine_cluster_protocol::HostedRunStatus::Target(RunStatus::Suspended { .. })
    ));
    assert_eq!(status.steering.steers, [recorded.steer]);
    let mut recovery = LocalRecoveryDocument {
        submission,
        workspace: root.path().into(),
        delivery_receipt: None,
        local_branch: None,
        invoking_workspace: None,
        request_digest: None,
        workspace_revision: None,
        resumed_from: None,
        delivery_run_id: None,
        successor_run_id: None,
        pending_restore: None,
    };
    backend
        .write_recovery_document(&params.run_id, &recovery)
        .assert_value();
    // A recoverable workspace uses the continuation claim, but recording still leaves it suspended.
    backend.run_steer(None, params.clone()).await.assert_value();
    let before = crate::v2_run_ledger::steering::all(&ledger, &params.run_id)
        .await
        .assert_value();
    assert_eq!(before.len(), 2);
    assert!(
        before.iter().all(|vector| matches!(&vector.content,
        openengine_cluster_protocol::RunVectorContent::Steer { text, .. } if text == &params.text))
    );
    let successor = RunId::new("0199f33f-3b44-7d21-9000-000000000072");
    recovery.successor_run_id = Some(successor.clone());
    backend
        .write_recovery_document(&params.run_id, &recovery)
        .assert_value();
    // An interrupted launch claimed a successor that never became a run: the direction is
    // still held on the suspended run, as status reports it.
    backend.run_steer(None, params.clone()).await.assert_value();
    let before = crate::v2_run_ledger::steering::all(&ledger, &params.run_id)
        .await
        .assert_value();
    assert_eq!(before.len(), 3);
    let _admitted = create_contract_run(
        &backend,
        &successor,
        &contract_submission("steer-local-successor"),
    )
    .await;
    let error = backend.run_steer(None, params.clone()).await.assert_error();
    assert!(error.to_string().contains(successor.as_str()), "{error}");
    assert_eq!(
        crate::v2_run_ledger::steering::all(&ledger, &params.run_id)
            .await
            .assert_value(),
        before
    );
}

#[tokio::test]
async fn steer_local_validation_and_run_state_refusals_never_record_direction() {
    let root = TestDirectory::new("steer-local-refusals");
    let backend = backend(&root);
    assert!(
        backend
            .run_steer(Some("target"), params())
            .await
            .assert_error()
            .to_string()
            .contains("cannot serve a named target")
    );
    let mut invalid = params();
    invalid.text = "\0".into();
    assert!(matches!(
        backend.run_steer(None, invalid).await,
        Err(NativeV2CliError::Usage(_))
    ));
    assert!(!root.child("state").exists());
    assert!(matches!(
        backend.run_steer(None, params()).await,
        Err(NativeV2CliError::RunNotFound { .. })
    ));

    for events in [
        vec![RunEvent::RunStarted, RunEvent::ForceStopRequested],
        vec![
            RunEvent::RunStarted,
            RunEvent::PrimaryGraphCompleted {
                result: TerminalResult::Succeeded {
                    output: serde_json::Value::Null,
                },
                auxiliary_pending: true,
                delivery_execution: None,
            },
        ],
        vec![
            RunEvent::RunStarted,
            RunEvent::Terminal {
                result: TerminalResult::Succeeded {
                    output: serde_json::Value::Null,
                },
            },
        ],
    ] {
        let root = TestDirectory::new("steer-local-state");
        let backend = self::backend(&root);
        let params = params();
        let ledger = create_contract_run(
            &backend,
            &params.run_id,
            &contract_submission("steer-local-state"),
        )
        .await;
        ledger.append(&params.run_id, events).await.assert_value();
        let error = backend.run_steer(None, params.clone()).await.assert_error();
        assert!(
            error.to_string().contains("before its graph ends"),
            "{error}"
        );
        assert!(
            crate::v2_run_ledger::steering::all(&ledger, &params.run_id)
                .await
                .assert_value()
                .is_empty()
        );
    }
}

struct SteeringController {
    ledger: SqliteRunLedger,
    expected: RunSteerParams,
    mode: &'static str,
    release: tokio::sync::Notify,
}

#[async_trait]
impl openengine_cluster_server::ClusterBackend for SteeringController {
    async fn initialize(
        &self,
        _: &openengine_cluster_server::ConnectionContext,
        _: openengine_cluster_protocol::InitializeParams,
    ) -> Result<
        openengine_cluster_protocol::InitializeResult,
        openengine_cluster_server::BackendError,
    > {
        Ok(openengine_cluster_protocol::InitializeResult::new(
            Default::default(),
            openengine_cluster_protocol::ClusterStatus::empty(),
        ))
    }

    async fn get(
        &self,
        _: &openengine_cluster_server::ConnectionContext,
        _: openengine_cluster_protocol::GetParams,
    ) -> Result<openengine_cluster_protocol::GetResult, openengine_cluster_server::BackendError>
    {
        Ok(openengine_cluster_protocol::GetResult::empty())
    }

    async fn run_status(
        &self,
        _: &openengine_cluster_server::ConnectionContext,
        _: RunStatusParams,
    ) -> Result<RunStatusResult, openengine_cluster_server::BackendError> {
        // A control request must reach the controller without waiting for a status probe.
        std::future::pending().await
    }

    async fn run_steer(
        &self,
        _: &openengine_cluster_server::ConnectionContext,
        params: RunSteerParams,
    ) -> Result<RunSteerResult, openengine_cluster_server::BackendError> {
        assert_eq!(params, self.expected);
        if self.mode == "refused" {
            return Err(openengine_cluster_server::BackendError::application(
                "invalid_request",
                "steer refused by controller",
                None,
            ));
        }
        let recorded = crate::v2_run_ledger::steering::record(&self.ledger, params)
            .await
            .assert_value();
        if self.mode == "timeout" {
            self.release.notified().await;
        }
        Ok(recorded)
    }
}

#[tokio::test]
async fn steer_local_controller_forwarding_preserves_receipts_refusals_and_timeout_uncertainty() {
    use std::io::Write;
    use crate::native_v2_portable_controller::availability::tests::ServedEndpoint;
    for mode in ["admitted", "running", "refused", "timeout"] {
        let root = TestDirectory::new("steer-local-wire");
        let backend = backend(&root);
        let params = params();
        let ledger = create_contract_run(
            &backend,
            &params.run_id,
            &contract_submission("steer-local-wire"),
        )
        .await;
        if mode != "admitted" {
            ledger
                .append(&params.run_id, vec![RunEvent::RunStarted])
                .await
                .assert_value();
        }
        let paths = backend.paths(&params.run_id).assert_value();
        let controller = Arc::new(SteeringController {
            ledger,
            expected: params.clone(),
            mode,
            release: Default::default(),
        });
        let _endpoint = ServedEndpoint::start(&paths, controller.clone());
        private_file(&paths.ready(), FileAccess::CreateNew)
            .assert_value()
            .write_all(
                &serde_json::to_vec(
                    &crate::native_v2_portable_controller::PortableControllerReady {
                        kind: "zeroshot.portable-controller-ready/v1".into(),
                        run_id: params.run_id.clone(),
                        socket: paths.socket(),
                        pid: std::process::id(),
                        lease_identity: None,
                    },
                )
                .assert_value(),
            )
            .assert_value();
        let started = Instant::now();
        let result = backend.run_steer(None, params.clone()).await;
        if mode == "timeout" {
            let error = result.assert_error().to_string();
            for expected in ["may have been recorded", "status", params.run_id.as_str()] {
                assert!(error.contains(expected), "{error}");
            }
            assert!(started.elapsed() < DEFAULT_READY_TIMEOUT + Duration::from_secs(2));
            controller.release.notify_one();
        } else if mode == "refused" {
            assert!(
                result
                    .assert_error()
                    .to_string()
                    .contains("steer refused by controller")
            );
        } else {
            assert_eq!(result.assert_value().steer.excerpt, params.text);
        }
        let vectors = crate::v2_run_ledger::steering::all(&controller.ledger, &params.run_id)
            .await
            .assert_value();
        assert_eq!(vectors.len(), usize::from(mode != "refused"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn steer_local_concurrent_steers_share_the_recovery_claim() {
    let root = TestDirectory::new("steer-local-concurrent");
    let backend = backend(&root);
    let params = params();
    let submission = contract_submission("steer-local-concurrent");
    let ledger = create_contract_run(&backend, &params.run_id, &submission).await;
    ledger
        .append(
            &params.run_id,
            vec![
                RunEvent::RunStarted,
                RunEvent::Suspended {
                    suspension: RunSuspension::Command {
                        node: openengine_cluster_protocol::NodeName::new("next").assert_value(),
                        created_at: UnixTimestampMillis::MIN,
                    },
                },
            ],
        )
        .await
        .assert_value();
    backend
        .write_recovery_document(
            &params.run_id,
            &LocalRecoveryDocument {
                submission,
                workspace: root.path().into(),
                delivery_receipt: None,
                local_branch: None,
                invoking_workspace: None,
                request_digest: None,
                workspace_revision: None,
                resumed_from: None,
                delivery_run_id: None,
                successor_run_id: None,
                pending_restore: None,
            },
        )
        .assert_value();
    let steers = (0..20)
        .map(|_| {
            let (backend, params) = (backend.clone(), params.clone());
            tokio::spawn(async move { backend.run_steer(None, params).await })
        })
        .collect::<Vec<_>>();
    for steer in steers {
        steer.await.assert_value().assert_value();
    }
    assert_eq!(
        crate::v2_run_ledger::steering::all(&ledger, &params.run_id)
            .await
            .assert_value()
            .len(),
        20
    );

    // A steer in flight leaves the workspace recoverable for status; resume and force-stop
    // wait for it instead of refusing.
    let shared = backend
        .try_recovery_claim(&params.run_id, true)
        .assert_value()
        .assert_value();
    assert!(
        backend
            .recovery_workspace_is_unclaimed(&params.run_id)
            .assert_value()
    );
    assert!(
        backend
            .try_recovery_claim(&params.run_id, false)
            .assert_value()
            .is_none()
    );
    let waiting = {
        let (backend, id) = (backend.clone(), params.run_id.clone());
        tokio::spawn(async move {
            backend
                .claim_recovery_workspace_within(&id, false, Duration::from_secs(10))
                .await
                .map(|_| ())
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!waiting.is_finished());
    drop(shared);
    waiting.await.assert_value().assert_value();
    // Behind steers alone even a caller with no patience of its own waits them out.
    let shared = backend
        .try_recovery_claim(&params.run_id, true)
        .assert_value()
        .assert_value();
    let waiting = {
        let (backend, id) = (backend.clone(), params.run_id.clone());
        tokio::spawn(async move {
            backend
                .claim_recovery_workspace_within(&id, false, Duration::ZERO)
                .await
                .map(|_| ())
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(shared);
    waiting.await.assert_value().assert_value();

    // A successor's preparation holds the claim alone: a steer waits, then says why.
    let preparing = backend
        .claim_recovery_workspace(&params.run_id)
        .assert_value();
    assert!(
        !backend
            .recovery_workspace_is_unclaimed(&params.run_id)
            .assert_value()
    );
    let error = backend.run_steer(None, params.clone()).await.assert_error();
    assert!(
        error.to_string().contains("already being resumed"),
        "{error}"
    );
    // Another resume gives up at its own bound: the holder may need a lock it waits under.
    assert!(
        backend
            .claim_recovery_workspace_within(&params.run_id, false, Duration::ZERO)
            .await
            .is_err()
    );
    drop(preparing);
    backend.run_steer(None, params.clone()).await.assert_value();
}
