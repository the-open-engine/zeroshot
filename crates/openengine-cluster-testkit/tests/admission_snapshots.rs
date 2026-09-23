use openengine_cluster_protocol::{
    Cursor, DispatchState, Generation, GetParams, OperationalStatus, Phase, RunId, StopMode,
    INTERNAL_ERROR_CODE, INVALID_PHASE,
};
use openengine_cluster_server::admission::{AdmissionSnapshot, ControlSnapshot, VerifiedSeed};
use openengine_cluster_server::lifecycle::{LifecycleSnapshot, TurnId};
use openengine_cluster_testkit::admission::{
    compiled_from_graph_fixture, graph_fixture, ScriptedOutcome,
};
use serde_json::json;

#[path = "admission_support/mod.rs"]
mod admission_support;
use admission_support::{client, committed, rpc_code};

fn valid_running_snapshot() -> AdmissionSnapshot {
    let graph = graph_fixture("worker", json!({"kind":"null"}));
    let run_id = RunId::new("run-fixture");
    let cursor = Cursor::new("cursor-fixture");
    AdmissionSnapshot {
        control: ControlSnapshot {
            spec: Some(graph.clone()),
            compiled_ir: Some(compiled_from_graph_fixture(&graph)),
            generation: Some(Generation::new(1).assert_value()),
            run_id: Some(run_id.clone()),
            phase: Phase::Running,
            cursor: Some(cursor.clone()),
        },
        seed: Some(VerifiedSeed {
            run_id,
            input: json!(null),
            cursor,
        }),
    }
}

#[tokio::test]
async fn admission_get_rejects_a_running_snapshot_without_a_verified_seed() {
    let graph = graph_fixture("worker", json!({"kind":"null"}));
    let compiled = compiled_from_graph_fixture(&graph);
    let (client, _, store) = client(vec![ScriptedOutcome::approve(compiled, vec![])]);
    client
        .apply(committed(graph, json!(null), 0, "create"))
        .await
        .assert_value();
    store.remove_active_seed_for_test().await;

    let error = client.get(GetParams::default()).await.assert_error();
    assert_eq!(rpc_code(error), INTERNAL_ERROR_CODE);
}

#[tokio::test]
async fn admission_initialize_and_get_reject_every_malformed_phase_snapshot() {
    type Corrupt = fn(&mut AdmissionSnapshot);
    let running_corruptions: [(&str, Corrupt); 10] = [
        ("missing spec", |snapshot| snapshot.control.spec = None),
        ("missing compiled IR", |snapshot| {
            snapshot.control.compiled_ir = None;
        }),
        ("missing generation", |snapshot| {
            snapshot.control.generation = None;
        }),
        ("zero committed generation", |snapshot| {
            snapshot.control.generation = Some(Generation::new(0).assert_value());
        }),
        ("missing run", |snapshot| snapshot.control.run_id = None),
        ("missing cursor", |snapshot| snapshot.control.cursor = None),
        ("missing seed", |snapshot| snapshot.seed = None),
        ("seed run mismatch", |snapshot| {
            snapshot.seed.as_mut().assert_value().run_id = RunId::new("other-run");
        }),
        ("seed cursor mismatch", |snapshot| {
            snapshot.seed.as_mut().assert_value().cursor = Cursor::new("other-cursor");
        }),
        ("seed input mismatch", |snapshot| {
            snapshot.seed.as_mut().assert_value().input = json!(true);
        }),
    ];
    let committed = valid_running_snapshot();
    let mut malformed = Vec::new();
    for (name, corrupt) in running_corruptions {
        let mut snapshot = committed.clone();
        corrupt(&mut snapshot);
        malformed.push((format!("running {name}"), snapshot));
    }

    for field in ["spec", "compiled IR", "generation", "run", "cursor"] {
        let mut snapshot = AdmissionSnapshot::default();
        match field {
            "spec" => snapshot.control.spec = committed.control.spec.clone(),
            "compiled IR" => {
                snapshot.control.compiled_ir = committed.control.compiled_ir.clone();
            }
            "generation" => snapshot.control.generation = committed.control.generation,
            "run" => snapshot.control.run_id = committed.control.run_id.clone(),
            "cursor" => snapshot.control.cursor = committed.control.cursor.clone(),
            other => assert!(
                matches!(other, "graph" | "ir" | "generation" | "run" | "cursor"),
                "unknown committed field {other}"
            ),
        }
        malformed.push((format!("empty with {field}"), snapshot));
    }

    let mut partial_admitting = AdmissionSnapshot::default();
    partial_admitting.control.phase = Phase::Admitting;
    partial_admitting.control.spec = committed.control.spec.clone();
    malformed.push(("partial admitting".into(), partial_admitting));

    for (name, snapshot) in malformed {
        let (client, _, store) = client(vec![]);
        store.replace_snapshot_for_test(snapshot).await;
        assert_eq!(
            rpc_code(client.initialize().await.assert_error()),
            INTERNAL_ERROR_CODE,
            "initialize accepted {name}"
        );
        assert_eq!(
            rpc_code(client.get(GetParams::default()).await.assert_error()),
            INTERNAL_ERROR_CODE,
            "get accepted {name}"
        );
    }
}

#[tokio::test]
async fn admission_admitting_snapshot_preserves_complete_empty_or_committed_state() {
    let mut empty = AdmissionSnapshot::default();
    empty.control.phase = Phase::Admitting;
    let mut committed = valid_running_snapshot();
    committed.control.phase = Phase::Admitting;

    for snapshot in [empty, committed] {
        let (client, _, store) = client(vec![]);
        store.replace_snapshot_for_test(snapshot).await;
        assert_eq!(
            client.initialize().await.assert_value().status.phase,
            Phase::Admitting
        );
        assert_eq!(
            client
                .get(GetParams::default())
                .await
                .assert_value()
                .status
                .phase,
            Phase::Admitting
        );
    }
}

fn natural_terminal(snapshot: &AdmissionSnapshot) -> LifecycleSnapshot {
    LifecycleSnapshot {
        operational: Some(OperationalStatus {
            dispatch_state: DispatchState::Stopped,
            stop_mode: None,
            ..OperationalStatus::default()
        }),
        latest_cursor: snapshot.control.cursor.clone(),
        ..LifecycleSnapshot::default()
    }
}

#[tokio::test]
async fn natural_terminal_snapshots_require_an_exact_empty_settlement_frontier() {
    let mut snapshot = valid_running_snapshot();
    snapshot.control.phase = Phase::Finished;
    let valid = natural_terminal(&snapshot);
    let (client, _, store) = client(vec![]);
    store.replace_snapshot_for_test(snapshot.clone()).await;
    store
        .replace_lifecycle_snapshot_for_test(valid.clone())
        .await;
    assert_eq!(
        client.initialize().await.assert_value().status.phase,
        Phase::Finished
    );

    let mut malformed = Vec::new();
    let mut missing_cursor = valid.clone();
    missing_cursor.latest_cursor = None;
    malformed.push(("missing latest cursor", missing_cursor));
    let mut wrong_cursor = valid.clone();
    wrong_cursor.latest_cursor = Some(Cursor::new("wrong-cursor"));
    malformed.push(("mismatched latest cursor", wrong_cursor));
    let mut active_work = valid.clone();
    active_work.operational.as_mut().assert_value().in_flight = 1;
    malformed.push(("active work", active_work));
    let mut stop_mode = valid.clone();
    stop_mode.operational.as_mut().assert_value().stop_mode = Some(StopMode::Force);
    malformed.push(("non-natural stop mode", stop_mode));
    let mut retry_frontier = valid;
    retry_frontier.pending_failed_frontier = Some(TurnId::new("failed"));
    malformed.push(("pending retry frontier", retry_frontier));

    for (case, lifecycle) in malformed {
        store.replace_lifecycle_snapshot_for_test(lifecycle).await;
        assert_eq!(
            rpc_code(client.get(GetParams::default()).await.assert_error()),
            INTERNAL_ERROR_CODE,
            "accepted {case}"
        );
    }
}

#[tokio::test]
async fn admission_get_fences_reads_to_the_exact_authoritative_cursor() {
    let snapshot = valid_running_snapshot();
    let cursor = snapshot.control.cursor.clone().assert_value();
    let (client, _, store) = client(vec![]);
    store.replace_snapshot_for_test(snapshot).await;

    let current = client
        .get(GetParams {
            at_cursor: Some(cursor.clone()),
        })
        .await
        .assert_value();
    assert_eq!(current.at_cursor, Some(cursor));
    assert_eq!(
        rpc_code(
            client
                .get(GetParams {
                    at_cursor: Some(Cursor::new("stale-cursor")),
                })
                .await
                .assert_error()
        ),
        INVALID_PHASE
    );
}

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
