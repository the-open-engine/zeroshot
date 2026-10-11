use openengine_cluster_protocol::WorkerErrorCode;

use super::*;

const NODE_INSTANCE: u64 = 1;

#[test]
fn loop_fact_reset_preserves_sibling_map_items_and_visit_markers() {
    let child: NodeName = "child".parse().expect("node name");
    let ancestor: NodeName = "ancestor".parse().expect("node name");
    let target_error = ControlKey {
        node: child.clone(),
        source: ControlSource::Error,
        field: None,
        map_indices: vec![0],
    };
    let sibling_signal = ControlKey {
        node: child.clone(),
        source: ControlSource::Signal,
        field: Some("verdict".to_owned()),
        map_indices: vec![1],
    };
    let ancestor_group = ControlKey {
        node: ancestor.clone(),
        source: ControlSource::Group,
        field: Some("terminated".to_owned()),
        map_indices: Vec::new(),
    };
    let visit = visit_key(&child, &[0]);
    let mut context = Context::new(Value::Null);
    context
        .controls
        .insert(target_error.clone(), "crash".to_owned());
    context
        .controls
        .insert(sibling_signal.clone(), "accepted".to_owned());
    context
        .controls
        .insert(ancestor_group.clone(), "done".to_owned());
    context.controls.insert(visit.clone(), "2".to_owned());
    let channels = Channels {
        output: Value::Null,
        signals: BTreeMap::new(),
        diagnostic: None,
    };
    context
        .channels
        .insert((child.clone(), vec![0]), channels.clone());
    context
        .channels
        .insert((child.clone(), vec![1]), channels.clone());
    context
        .channels
        .insert((ancestor.clone(), Vec::new()), channels);

    clear_scoped_runtime_facts(&mut context, &BTreeSet::from([child.clone()]), &[0]);

    assert!(!context.controls.contains_key(&target_error));
    assert_eq!(
        context.controls.get(&sibling_signal).map(String::as_str),
        Some("accepted")
    );
    assert_eq!(
        context.controls.get(&ancestor_group).map(String::as_str),
        Some("done")
    );
    assert_eq!(context.controls.get(&visit).map(String::as_str), Some("2"));
    assert!(!context.channels.contains_key(&(child.clone(), vec![0])));
    assert!(context.channels.contains_key(&(child, vec![1])));
    assert!(context.channels.contains_key(&(ancestor, Vec::new())));
}

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
