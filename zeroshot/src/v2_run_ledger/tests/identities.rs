use super::*;

#[test]
fn node_instance_reservations_survive_replay_and_empty_successor_history() {
    let mut source = projected_snapshot("predecessor");
    let through = 17;
    apply_event(&mut source, &RunEvent::NodeInstancesReserved { through }, 1).assert_value();
    assert_eq!(source.reserved_node_instance(), through);
    let mut successor = projected_snapshot("successor");
    apply_event(
        &mut successor,
        &RunEvent::NodeInstancesReserved {
            through: source.reserved_node_instance(),
        },
        1,
    )
    .assert_value();
    assert!(successor.executions.is_empty());
    assert!(successor.execution_seed.is_empty());
    assert_eq!(successor.reserved_node_instance(), through);
    let mut replay = successor.replay_seed();
    apply_event(&mut replay, &RunEvent::NodeInstancesReserved { through }, 1).assert_value();
    assert_eq!(replay, successor);
    apply_event(&mut successor, &RunEvent::RunStarted, 2).assert_value();
    assert!(
        apply_event(
            &mut successor,
            &RunEvent::NodeInstancesReserved { through },
            3
        )
        .is_err()
    );
}

#[test]
fn source_reservation_includes_used_ids_and_old_snapshots_default_to_zero() {
    let mut source = projected_snapshot("predecessor");
    let mut old = serde_json::to_value(&source).assert_value();
    old.as_object_mut()
        .assert_value()
        .remove("nodeInstanceFloor");
    let restored: RunSnapshot = serde_json::from_value(old).assert_value();
    assert_eq!(restored.node_instance_floor, 0);
    apply_event(
        &mut source,
        &RunEvent::NodeInstancesReserved { through: 3 },
        1,
    )
    .assert_value();
    apply_event(&mut source, &RunEvent::RunStarted, 2).assert_value();
    let mut worker = reference(&source.run_id, 9);
    worker.node_instance = NodeInstanceId::new(5).assert_value();
    apply_event(&mut source, &started(worker), 3).assert_value();
    assert_eq!(source.reserved_node_instance(), 5);
}

#[tokio::test]
async fn node_instance_reservation_is_durable_in_sqlite_and_cannot_move_backwards() {
    let root = tempfile::tempdir().assert_value();
    let path = root.path().join("runs.sqlite3");
    let ledger = SqliteRunLedger::open(&path).assert_value();
    let run = RunId::new("identity-floor");
    ledger
        .create_or_get(create(run.as_str(), "identity-floor", 'a'))
        .await
        .assert_value();
    ledger
        .append(&run, vec![RunEvent::NodeInstancesReserved { through: 23 }])
        .await
        .assert_value();
    assert!(
        ledger
            .append(&run, vec![RunEvent::NodeInstancesReserved { through: 0 }])
            .await
            .is_err()
    );
    drop(ledger);
    let reopened = SqliteRunLedger::open(&path).assert_value();
    let source = reopened.get(&run).await.assert_value().assert_value();
    assert_eq!(source.snapshot.reserved_node_instance(), 23);
}

#[test]
fn inherited_network_origins_and_receipts_reserve_instances_monotonically() {
    use openengine_cluster_protocol::{NetworkSteer, RunVectorContent, SteerOrigin, SteeringMode};
    let mut source = projected_snapshot("imported-vectors");
    for (sequence, instance) in [(1, 9), (3, 7)] {
        let event = crate::v2_run_ledger::vector_event(
            &source.run_id,
            RunVectorContent::Steer {
                text: "peer guidance".into(),
                mode: SteeringMode::Once,
                network: Some(NetworkSteer {
                    networks: vec![NodeName::new("team").assert_value()],
                    origin: SteerOrigin {
                        node: NodeName::new("worker").assert_value(),
                        node_instance: instance,
                        execution: reference(&source.run_id, 1)
                            .public_execution()
                            .assert_value(),
                    },
                }),
            },
        )
        .assert_value();
        apply_event(&mut source, &event, sequence).assert_value();
        if sequence == 1 {
            assert_eq!(source.node_instance_floor, 9);
            let RunEvent::Vector { vector } = event else {
                panic!("expected vector")
            };
            let receipt = crate::v2_run_ledger::vector_event(
                &source.run_id,
                RunVectorContent::SteerReceipt {
                    steer_id: vector.id,
                    conversation: "saved".into(),
                    execution: reference(&source.run_id, 2)
                        .public_execution()
                        .assert_value(),
                    node_instance: 15,
                },
            )
            .assert_value();
            apply_event(&mut source, &receipt, 2).assert_value();
        }
        assert_eq!(source.node_instance_floor, 15);
    }
}
