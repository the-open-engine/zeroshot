use openengine_cluster_protocol::WorkerErrorCode;

use super::*;

const NODE_INSTANCE: u64 = 1;

fn execution(
    execution: u64,
    attempt: u64,
    dispatch_position: u64,
    state: DurableExecutionState,
) -> DurableExecution {
    DurableExecution {
        dispatch_position: HistoryPosition::new(dispatch_position).expect("valid position"),
        node_instance: NodeInstanceId::new(NODE_INSTANCE).expect("valid node instance"),
        execution: ExecutionId::new(execution).expect("valid execution"),
        occurrence: StructuralOccurrence {
            node: "loop_work".parse().expect("valid node name"),
            map_indices: Vec::new(),
        },
        attempt: PositiveInteger::new(attempt).expect("positive attempt"),
        input: Value::Null,
        state,
    }
}

#[test]
fn native_history_accepts_fresh_visit_after_void_but_rejects_retry() {
    let voided = execution(
        1,
        INITIAL_ATTEMPT,
        1,
        DurableExecutionState::Voided {
            position: HistoryPosition::new(2).expect("valid position"),
            reason: ExecutionVoidReason::ParallelJoin,
        },
    );
    let fresh_visit = execution(2, INITIAL_ATTEMPT, 3, DurableExecutionState::Active);
    assert_eq!(
        validate_native_attempt_lineage(&[voided.clone(), fresh_visit]),
        Ok(())
    );

    let invalid_retry = execution(2, INITIAL_ATTEMPT + 1, 3, DurableExecutionState::Active);
    assert_eq!(
        validate_native_attempt_lineage(&[voided, invalid_retry]),
        Err(ReducerError::InconsistentHistory)
    );
}

#[test]
fn native_history_accepts_retry_after_settled_crash() {
    let failed = execution(
        1,
        INITIAL_ATTEMPT,
        1,
        DurableExecutionState::Settled {
            position: HistoryPosition::new(2).expect("valid position"),
            outcome: WorkerOutcome::declared_failure(WorkerErrorCode::Crash),
        },
    );
    let retry = execution(2, INITIAL_ATTEMPT + 1, 3, DurableExecutionState::Active);
    assert_eq!(validate_native_attempt_lineage(&[failed, retry]), Ok(()));
}

#[test]
fn native_history_rejects_retry_after_non_crash_error() {
    let timed_out = execution(
        1,
        INITIAL_ATTEMPT,
        1,
        DurableExecutionState::Settled {
            position: HistoryPosition::new(2).expect("valid position"),
            outcome: WorkerOutcome::declared_failure(WorkerErrorCode::Timeout),
        },
    );
    let retry = execution(2, INITIAL_ATTEMPT + 1, 3, DurableExecutionState::Active);
    assert_eq!(
        validate_native_attempt_lineage(&[timed_out, retry]),
        Err(ReducerError::InconsistentHistory)
    );
}
