use super::*;
use super::runtime_lanes::RecordingTarget;
use super::super::fixtures::{
    direct_target, hosted_target, exact_run_request, run_request, temp_root, FakeDialer,
    MemoryRegistry,
};
use openengine_cluster_protocol::{
    RunId, RunListParams, RunStatusParams, RunWatchParams, TargetAuthentication,
    TargetDiscoveryDocument,
};
use zeroshot_engine::native_v2_cli::{CliSubscription, CliSubscriptionItem, NativeV2CliBackend};

#[tokio::test]
async fn wire_contract_mismatch_stops_submission_and_observation_at_http_discovery() {
    for variant in ["direct", "hosted-target", "hosted-runs"] {
        let server = RecordingTarget::spawn(move |request, origin, _| {
            assert_eq!(request.path, "/.well-known/zeroshot-native-v2");
            let mut wire = if variant == "direct" {
                serde_json::to_value(TargetDiscoveryDocument::direct(TargetAuthentication::None))
                    .assert_value()
            } else {
                serde_json::from_str::<serde_json::Value>(&hosted_discovery(origin)).assert_value()
            };
            if variant == "hosted-runs" {
                wire["extensions"]["hosted_runs"]["kind"] = json!("zeroshot.hosted-runs/v2");
            } else {
                wire["kind"] = json!("zeroshot.native-v2-target/v3");
            }
            ("200 OK", wire.to_string())
        })
        .await;
        let target = if variant == "direct" {
            direct_target(&server.origin)
        } else {
            hosted_target("prod", &server.origin)
        };
        let root = temp_root();
        let (_, authority) = test_authority(&root);
        let registry = MemoryRegistry::default();
        registry.insert(target.clone()).assert_value();
        let dialer = FakeDialer::default();
        let connector = NativeV2TargetConnector::new(registry, authority, dialer.clone());
        let error = connector
            .submit(&target.name, run_request())
            .await
            .err()
            .assert_value();
        assert_incompatibility(&error.to_string());
        let backend = zeroshot_engine::native_v2_cli::oecp::NamedTargetCliBackend::new(connector);
        let error = backend
            .run_status(
                Some(&target.name),
                RunStatusParams::new(RunId::new("active")),
            )
            .await
            .err()
            .assert_value();
        assert_incompatibility(&error.to_string());
        let error = backend
            .run_list(Some(&target.name), RunListParams::default())
            .await
            .err()
            .assert_value();
        assert_incompatibility(&error.to_string());
        let error = backend
            .run_watch(
                Some(&target.name),
                RunWatchParams {
                    part_id: None,
                    run_id: RunId::new("active"),
                    from_cursor: None,
                },
            )
            .await
            .err()
            .assert_value();
        assert_incompatibility(&error.to_string());
        assert!(dialer.sessions.lock().assert_value().is_empty());
        let requests = server.stop();
        assert_eq!(
            requests.len(),
            4,
            "each operation discovers once; no submission replay"
        );
        assert!(
            requests.iter().all(|request| request.method == "GET"
                && request.path == "/.well-known/zeroshot-native-v2")
        );
    }
}

fn assert_incompatibility(message: &str) {
    assert!(message.contains("incompatible"), "{message}");
    assert!(
        message.contains("upgrade the Zeroshot client and target together"),
        "{message}"
    );
    assert!(!message.contains("startedAt") && !message.contains("recordedAt"));
}

/// Produce the HTTP payloads from the runtime ledger/projector, including a finished notification.
async fn produced_observation() -> (serde_json::Value, serde_json::Value, String) {
    use openengine_cluster_protocol::{
        HostedRunStatusResult, HostedRunWatchEventNotification, IdempotencyKey, NodeName,
        PositiveInteger, Sha256Digest, TerminalResult, UnixTimestampMillis,
    };
    use zeroshot_engine::native_v2_contract::{ExecutionRef, ExecutionId, NodeInstanceId};
    use zeroshot_engine::full_v1_reducer::StructuralOccurrence;
    use zeroshot_engine::v2_run_ledger::{CreateRun, RunEvent, RunLedger, fake::FakeRunLedger};
    let now = Arc::new(std::sync::atomic::AtomicU64::new(10_000));
    let clock = now.clone();
    let ledger = Arc::new(FakeRunLedger::new().with_wall_clock(Arc::new(move || {
        UnixTimestampMillis::new(clock.load(Ordering::SeqCst)).assert_value()
    })));
    let admitted = zeroshot_engine::native_v2_admission::NativeV2Admission
        .admit(exact_run_request().submission)
        .await
        .assert_value();
    let run_id = RunId::new("active");
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            admitted: admitted.clone(),
            submission_key: IdempotencyKey::new("wire").assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
        })
        .await
        .assert_value();
    let node = NodeName::new("done").assert_value();
    let reference = ExecutionRef {
        run_id: run_id.clone(),
        node: node.clone(),
        execution: ExecutionId::new(37).assert_value(),
        node_instance: NodeInstanceId::new(11).assert_value(),
    };
    ledger
        .append(
            &run_id,
            vec![
                RunEvent::RunStarted,
                RunEvent::NodeStarted {
                    reference: reference.clone(),
                    occurrence: StructuralOccurrence {
                        node,
                        map_indices: vec![2],
                    },
                    attempt: PositiveInteger::new(1).assert_value(),
                    input: serde_json::Value::Null,
                },
            ],
        )
        .await
        .assert_value();
    let service =
        zeroshot_engine::native_v2_observability::NativeV2Observability::new(ledger.clone());
    let active = HostedRunStatusResult::from(
        service
            .status(RunStatusParams::new(run_id.clone()))
            .await
            .assert_value(),
    );
    let (_, mut watch) = service
        .watch(RunWatchParams {
            part_id: None,
            run_id: run_id.clone(),
            from_cursor: None,
        })
        .await
        .assert_value();
    let mut events = watch.read_available().await.assert_value();
    now.store(20_000, Ordering::SeqCst);
    ledger
        .append(
            &run_id,
            vec![
                RunEvent::NodeCompleted {
                    completion: zeroshot_engine::native_v2_contract::NodeCompletion {
                        reference,
                        outcome: openengine_cluster_protocol::WorkerOutcome::Verified {
                            output: serde_json::Value::Null,
                            artifacts: Vec::new(),
                        },
                    },
                },
                RunEvent::Terminal {
                    result: TerminalResult::Succeeded {
                        output: serde_json::Value::Null,
                    },
                },
            ],
        )
        .await
        .assert_value();
    events.extend(watch.read_available().await.assert_value());
    let finished_id = RunId::new("finished");
    ledger
        .create_or_get(CreateRun {
            run_id: finished_id.clone(),
            admitted: admitted.clone(),
            submission_key: IdempotencyKey::new("finished-wire").assert_value(),
            submission_digest: Sha256Digest::new("b".repeat(64)).assert_value(),
        })
        .await
        .assert_value();
    ledger
        .append(
            &finished_id,
            vec![
                RunEvent::RunStarted,
                RunEvent::Terminal {
                    result: TerminalResult::Succeeded {
                        output: serde_json::Value::Null,
                    },
                },
            ],
        )
        .await
        .assert_value();
    let finished = HostedRunStatusResult::from(
        service
            .status(RunStatusParams::new(finished_id))
            .await
            .assert_value(),
    );
    let pending_id = RunId::new("pending");
    ledger
        .create_or_get(CreateRun {
            run_id: pending_id.clone(),
            admitted,
            submission_key: IdempotencyKey::new("pending-wire").assert_value(),
            submission_digest: Sha256Digest::new("c".repeat(64)).assert_value(),
        })
        .await
        .assert_value();
    let pending = HostedRunStatusResult::from(
        service
            .status(RunStatusParams::new(pending_id))
            .await
            .assert_value(),
    );
    let list = json!({"runs": [pending, &active, finished]});
    let active = serde_json::to_value(active).assert_value();
    let stream = events
        .into_iter()
        .map(|event| {
            json!({"type":"event", "event":HostedRunWatchEventNotification::from(event)})
                .to_string()
                + "\n"
        })
        .collect();
    (active, list, stream)
}

#[tokio::test]
async fn matching_hosted_wire_contract_preserves_producer_status_mixed_list_and_watch_timing() {
    let (status, list, stream) = produced_observation().await;
    assert_eq!(status["status"]["activeExecutions"][0]["startedAt"], 10_000);
    assert_eq!(list["runs"][0]["status"]["phase"], "admitted");
    assert_eq!(list["runs"][1]["status"]["phase"], "running");
    assert_eq!(list["runs"][2]["status"]["phase"], "finished");
    let expected_status = status.clone();
    let expected_list = list.clone();
    let expected_events = stream
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).assert_value()["event"].clone())
        .collect::<Vec<_>>();
    let server = RecordingTarget::spawn(move |request, origin, index| {
        let body = match request.path.as_str() {
            "/.well-known/zeroshot-native-v2" => hosted_discovery(origin),
            "/oauth/metadata" => oauth_metadata(origin),
            "/oauth/token" => token_response(index),
            "/session" => {
                json!({"kind":"openengine.target-session/v1", "organization_id":"organization-1"})
                    .to_string()
            }
            "/native-v2/runs/active" => status.to_string(),
            "/native-v2/runs" => list.to_string(),
            "/native-v2/runs/active/watch?from_cursor=cloud%3A7" => {
                return ("200 OK", stream.clone());
            }
            "/native-v2/runs/active?partId=active" => status.to_string(),
            "/native-v2/runs/active/logs?partId=active" => return ("200 OK", String::new()),
            "/native-v2/runs/active/force" => {
                assert_eq!(request.body, r#"{"partId":"active"}"#);
                status.to_string()
            }
            path => panic!("unexpected hosted request: {path}"),
        };
        ("200 OK", body)
    })
    .await;
    let root = temp_root();
    let (credentials, authority) = test_authority(&root);
    let target = hosted_target("prod", &server.origin);
    credentials
        .set(&target.id, "refresh-0")
        .await
        .assert_value();
    let registry = MemoryRegistry::default();
    registry.insert(target).assert_value();
    let dialer = FakeDialer::default();
    let backend = zeroshot_engine::native_v2_cli::oecp::NamedTargetCliBackend::new(
        NativeV2TargetConnector::new(registry, authority, dialer.clone()),
    );
    let status = backend
        .run_status(Some("prod"), RunStatusParams::new(RunId::new("active")))
        .await
        .assert_value();
    assert_eq!(serde_json::to_value(status).assert_value(), expected_status);
    let list = backend
        .run_list(Some("prod"), RunListParams::default())
        .await
        .assert_value();
    assert_eq!(serde_json::to_value(list).assert_value(), expected_list);
    let mut watch = backend
        .run_watch(
            Some("prod"),
            RunWatchParams {
                part_id: None,
                run_id: RunId::new("active"),
                from_cursor: Some(openengine_cluster_protocol::Cursor::new("cloud:7")),
            },
        )
        .await
        .assert_value();
    let selected = RunStatusParams {
        run_id: RunId::new("active"),
        part_id: Some(RunId::new("active")),
    };
    backend
        .run_logs(
            Some("prod"),
            openengine_cluster_protocol::RunLogsParams {
                run_id: selected.run_id.clone(),
                part_id: selected.part_id.clone(),
                from_cursor: None,
                execution: None,
            },
        )
        .await
        .assert_value();
    backend
        .run_force(
            Some("prod"),
            openengine_cluster_protocol::RunForceParams {
                run_id: selected.run_id,
                part_id: selected.part_id,
            },
        )
        .await
        .assert_value();
    assert!(dialer.sessions.lock().assert_value().is_empty());
    let mut active_seen = false;
    let mut finished_seen = false;
    let mut index = 0;
    while let Some(item) = watch.next().await.assert_value() {
        if let CliSubscriptionItem::Event(event) = item {
            let value = serde_json::to_value(event).assert_value();
            assert_eq!(value, expected_events[index]);
            index += 1;
            if value["status"]["activeExecutions"][0]["startedAt"] == 10_000 {
                active_seen = true;
            }
            if value["status"]["phase"] == "finished" {
                assert_eq!(value["recordedAt"], 20_000);
                finished_seen = true;
            }
        }
    }
    assert!(active_seen && finished_seen);
    assert_eq!(index, expected_events.len());
    assert_eq!(
        server
            .stop()
            .iter()
            .filter(|request| request.path == "/oauth/token")
            .count(),
        1
    );
}
