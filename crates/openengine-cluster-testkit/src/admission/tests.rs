use super::*;
use crate::assertions::{AssertError, AssertValue};
use openengine_cluster_protocol::{admission_fingerprint, DeleteResult, RequestFingerprint};
use openengine_cluster_server::lifecycle::MutationReceipt;
use serde_json::json;

fn fingerprint(case: &str) -> RequestFingerprint {
    admission_fingerprint("apply", &json!({"case": case})).assert_value()
}

fn proposal(name: &str, key: &str, case: &str, input: Option<Value>) -> CommitProposal {
    let graph = graph_fixture(name, json!({"kind": "null"}));
    CommitProposal {
        compiled_ir: compiled_from_graph_fixture(&graph),
        graph,
        input,
        if_generation: None,
        idempotency_key: IdempotencyKey::new(key).assert_value(),
        fingerprint: fingerprint(case),
    }
}

#[test]
fn commit_replay_rejects_conflicts_wrong_receipts_and_invalid_phases() {
    let cancellation = CancellationSignal::default();
    let original = proposal("worker", "stable", "original", Some(Value::Null));
    let mut state = StoreState::default();
    let receipt = state.commit(original.clone(), &cancellation).assert_value();
    let replay = state.commit(original.clone(), &cancellation).assert_value();
    assert!(replay.deduped);
    assert_eq!(replay.run_id, receipt.run_id);

    let mut conflicting = original.clone();
    conflicting.fingerprint = fingerprint("different");
    assert_eq!(
        state.commit(conflicting, &cancellation).assert_error(),
        StoreError::IdempotencyReuse
    );

    let wrong_receipt = proposal("worker", "wrong-kind", "wrong-kind", Some(Value::Null));
    state.idempotency_records.insert(
        wrong_receipt.idempotency_key.clone(),
        IdempotencyRecord {
            fingerprint: wrong_receipt.fingerprint.clone(),
            receipt: MutationReceipt::Delete(DeleteResult {
                deleted: false,
                phase: Phase::Empty,
                generation: None,
                run_id: None,
                at_cursor: None,
                deduped: false,
            }),
        },
    );
    assert_eq!(
        state.commit(wrong_receipt, &cancellation).assert_error(),
        StoreError::IdempotencyReuse
    );

    let mut finished = StoreState::default();
    finished.control.phase = Phase::Finished;
    assert!(matches!(
        finished.commit(
            proposal("worker", "finished", "finished", Some(Value::Null)),
            &cancellation
        ),
        Err(StoreError::InvalidPhase {
            current: Phase::Finished
        })
    ));
}

#[test]
fn commit_validates_input_cancellation_and_active_writer_exclusion_atomically() {
    let cancellation = CancellationSignal::default();
    let mut missing_input = StoreState::default();
    assert!(matches!(
        missing_input.commit(
            proposal("worker", "missing", "missing", None),
            &cancellation
        ),
        Err(StoreError::SchemaViolation(_))
    ));

    let cancelled = CancellationSignal::default();
    cancelled.cancel();
    let mut cancelled_state = StoreState::default();
    assert_eq!(
        cancelled_state
            .commit(
                proposal("worker", "cancelled", "cancelled", Some(Value::Null)),
                &cancelled
            )
            .assert_error(),
        StoreError::Cancelled
    );

    let mut active = StoreState::default();
    active
        .commit(
            proposal("original", "create", "create", Some(Value::Null)),
            &cancellation,
        )
        .assert_value();
    active.leases.insert(
        LeaseId::new("active-lease"),
        ActiveLease {
            turn_id: TurnId::new("active-turn"),
            cancellation: CancellationSignal::default(),
        },
    );
    let mut changed = proposal("changed", "changed", "changed", Some(Value::Null));
    changed.if_generation = active.control.generation;
    assert!(matches!(
        active.commit(changed, &cancellation),
        Err(StoreError::InvalidPhase {
            current: Phase::Running
        })
    ));
}
