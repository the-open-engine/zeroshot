use super::*;

#[test]
fn primary_completion_requires_quiescence_is_immutable_and_prevents_new_dispatch() {
    let (mut snapshot, active) = active_snapshot("primary-latch");
    let primary = RunEvent::PrimaryGraphCompleted {
        result: TerminalResult::Succeeded {
            output: Value::Null,
        },
        auxiliary_pending: true,
    };
    assert_invalid_projection(
        &mut snapshot,
        primary.clone(),
        "primary graph still has active executions",
    );
    apply_event(&mut snapshot, &completed(active, Value::Null), 3).assert_value();
    apply_event(&mut snapshot, &primary, 4).assert_value();
    assert!(snapshot.terminal.is_none());
    assert_eq!(snapshot.phase, RunPhase::Running);
    assert!(
        snapshot
            .primary_graph
            .as_ref()
            .assert_value()
            .auxiliary_pending
    );
    assert_invalid_projection(&mut snapshot, primary, "primary graph already completed");
    let next = reference(&snapshot.run_id, 2);
    assert_invalid_projection(&mut snapshot, started(next), "run is not dispatchable");
    assert!(snapshot.replay_seed().primary_graph.is_none());
    apply_event(&mut snapshot, &RunEvent::ForceStopRequested, 5).assert_value();
    apply_event(
        &mut snapshot,
        &RunEvent::Terminal {
            result: TerminalResult::Succeeded {
                output: Value::Null,
            },
        },
        6,
    )
    .assert_value();
    assert!(
        !snapshot
            .primary_graph
            .as_ref()
            .assert_value()
            .auxiliary_pending
    );
}

#[test]
fn primary_completion_is_optional_in_retained_legacy_snapshot_json() {
    let snapshot = projected_snapshot("legacy-primary");
    let mut encoded = serde_json::to_value(&snapshot).assert_value();
    encoded
        .as_object_mut()
        .assert_value()
        .remove("primaryGraph");
    let decoded: RunSnapshot = serde_json::from_value(encoded).assert_value();
    assert!(decoded.primary_graph.is_none());
    assert_eq!(decoded, snapshot);
}
