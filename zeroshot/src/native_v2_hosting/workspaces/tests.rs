use super::*;
use crate::full_v1_reducer::{DurableExecutionState, HistoryPosition, StructuralOccurrence};
use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeCompletion, NodeInstanceId};
use crate::native_v2_request::RequestOutcome;
use crate::v2_run_ledger::{MAX_EVENT_BYTES, RunEvent, RunLedgerError};
use openengine_cluster_protocol::{
    MAX_SAFE_GENERATION, RunResumeFrom, RunSuspension, RunVector, UnixTimestampMillis, VectorId,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

fn reference(node: NodeName) -> ExecutionRef {
    ExecutionRef {
        run_id: RunId::new("r".repeat(64)),
        node,
        node_instance: NodeInstanceId::new(u64::MAX).assert_value(),
        execution: ExecutionId::new(u64::MAX).assert_value(),
    }
}

fn events(suspension: &RunSuspension, answer: &RunResumeFrom) -> [RunEvent; 2] {
    let RunSuspension::Request { node, request, .. } = suspension else {
        panic!("request fixture required");
    };
    let resolution = crate::native_v2_request::resolve(
        suspension,
        Some(answer),
        UnixTimestampMillis::new(1999).assert_value(),
    )
    .assert_value();
    let reference = reference(node.clone());
    [
        RunEvent::NodeCompleted {
            completion: NodeCompletion {
                reference: reference.clone(),
                outcome: resolution.outcome(),
            },
        },
        RunEvent::Vector {
            vector: RunVector {
                id: VectorId::new("v".repeat(64)).assert_value(),
                run_id: reference.run_id,
                created_at: UnixTimestampMillis::new(MAX_SAFE_GENERATION).assert_value(),
                content: crate::native_v2_request::resolution_vector(request, &resolution),
            },
        },
    ]
}

fn sized_answer(event: usize, extra: usize) -> (RunSuspension, RunResumeFrom) {
    let suspension: RunSuspension = serde_json::from_value(json!({
        "reason":"request", "node":"n".repeat(if event == 0 {128} else {1}), "createdAt":1000,
        "request":{"id":"q".repeat(if event == 0 {1} else {64}), "text":"Review?",
            "signals":{}, "output":{"kind":"record","fields":{
                "feedback":{"required":true,"type":{"kind":"string"}}
            }}, "expiresAt":2000}
    }))
    .assert_value();
    let mut answer = RunResumeFrom::Answer {
        signals: Default::default(),
        output: json!({"feedback":""}),
    };
    let overhead = serde_json::to_vec(&events(&suspension, &answer)[event])
        .assert_value()
        .len();
    if let RunResumeFrom::Answer { output, .. } = &mut answer {
        output["feedback"] = json!("x".repeat(MAX_EVENT_BYTES - overhead + extra));
    }
    let sizes =
        events(&suspension, &answer).map(|value| serde_json::to_vec(&value).assert_value().len());
    assert_eq!(sizes[event], MAX_EVENT_BYTES + extra);
    assert!(sizes[1 - event] < MAX_EVENT_BYTES, "events: {sizes:?}");
    (suspension, answer)
}

#[test]
fn host_acceptance_rejects_oversized_completion_and_answer_vector_events() {
    for event in 0..2 {
        let (suspension, answer) = sized_answer(event, 1);
        assert_eq!(
            crate::v2_run_ledger::validate_event(&events(&suspension, &answer)[event]),
            Err(RunLedgerError::EventTooLarge),
        );
        let error = HostedRequestContinuation::validate(
            suspension,
            answer,
            UnixTimestampMillis::new(1999).assert_value(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            format!("$.output: {}", RunLedgerError::EventTooLarge)
        );
    }
}

#[test]
fn host_acceptance_at_each_event_ceiling_still_settles_the_answer() {
    for event in 0..2 {
        let (suspension, answer) = sized_answer(event, 0);
        let RunSuspension::Request { node, .. } = &suspension else {
            panic!("request fixture required");
        };
        let reference = reference(node.clone());
        let continuation = HostedRequestContinuation::validate(
            suspension.clone(),
            answer.clone(),
            UnixTimestampMillis::new(1999).assert_value(),
        )
        .assert_value();
        let mut workspace = HostedWorkspace {
            resumed_from: Some(RunId::new("predecessor")),
            vector_seed: vec![],
            comment_publications: None,
            checkpoints: Arc::new(
                crate::native_v2_supervisor::checkpoints::DeliveryCheckpointStore::new(
                    Default::default(),
                ),
            ),
            execution_seed: vec![DurableExecution {
                delivery_state: None,
                dispatch_position: HistoryPosition::new(1).assert_value(),
                node_instance: reference.node_instance,
                execution: reference.execution,
                occurrence: StructuralOccurrence {
                    node: node.clone(),
                    map_indices: vec![],
                },
                attempt: openengine_cluster_protocol::PositiveInteger::new(1).assert_value(),
                input: json!({"text":"Review?"}),
                state: DurableExecutionState::Active,
            }],
            request_continuation: Some(continuation),
            delivery_run_id: None,
        };
        workspace.settle_request(&reference.run_id).assert_value();
        assert_eq!(workspace.vector_seed.len(), 1);
        let resolution = crate::native_v2_request::resolve(
            &suspension,
            Some(&answer),
            UnixTimestampMillis::new(1999).assert_value(),
        )
        .assert_value();
        assert!(matches!(&workspace.execution_seed[0].state,
            DurableExecutionState::Settled {outcome, ..} if *outcome == resolution.outcome()));
        assert_eq!(
            workspace.vector_seed[0].content,
            crate::native_v2_request::resolution_vector(
                match &suspension {
                    RunSuspension::Request { request, .. } => request,
                    _ => unreachable!(),
                },
                &resolution,
            )
        );
    }
}

fn request() -> RunSuspension {
    serde_json::from_value(json!({
        "reason":"request", "node":"approval", "createdAt":1000,
        "request":{"id":"question", "text":"Review?", "signals":{},
            "output":{"kind":"record","fields":{}}, "expiresAt":2000}
    }))
    .assert_value()
}

fn restored_workspace() -> HostedWorkspace {
    HostedWorkspace {
        resumed_from: Some(RunId::new("predecessor")),
        vector_seed: vec![],
        comment_publications: None,
        checkpoints: Arc::new(
            crate::native_v2_supervisor::checkpoints::DeliveryCheckpointStore::new(
                Default::default(),
            ),
        ),
        execution_seed: vec![DurableExecution {
            delivery_state: None,
            dispatch_position: HistoryPosition::new(1).assert_value(),
            node_instance: NodeInstanceId::new(1).assert_value(),
            execution: ExecutionId::new(1).assert_value(),
            occurrence: StructuralOccurrence {
                node: NodeName::new("approval").assert_value(),
                map_indices: vec![],
            },
            attempt: openengine_cluster_protocol::PositiveInteger::new(1).assert_value(),
            input: json!({"text":"Review?"}),
            state: DurableExecutionState::Active,
        }],
        request_continuation: Some(
            HostedRequestContinuation::validate(
                request(),
                RunResumeFrom::Expiry {},
                UnixTimestampMillis::new(2000).assert_value(),
            )
            .assert_value(),
        ),
        delivery_run_id: None,
    }
}

#[test]
fn host_acceptance_requires_explicit_expiry_and_checks_answer_shape_before_admission() {
    let answer = RunResumeFrom::Answer {
        signals: Default::default(),
        output: json!({"unknown":true}),
    };
    let error = HostedRequestContinuation::validate(
        request(),
        answer.clone(),
        UnixTimestampMillis::new(1999).assert_value(),
    )
    .unwrap_err();
    assert!(
        error.contains("$.output") && error.contains("undeclared field unknown"),
        "{error}"
    );
    let error = HostedRequestContinuation::validate(
        request(),
        answer,
        UnixTimestampMillis::new(2000).assert_value(),
    )
    .unwrap_err();
    assert_eq!(error, "request expired; select expiry explicitly");
    let error = HostedRequestContinuation::validate(
        request(),
        RunResumeFrom::Expiry {},
        UnixTimestampMillis::new(1999).assert_value(),
    )
    .unwrap_err();
    assert!(error.contains("not expired"), "{error}");
}

#[test]
fn restored_request_rejects_missing_predecessor_and_ambiguous_or_foreign_history() {
    for invalid in 0..5 {
        let mut workspace = restored_workspace();
        match invalid {
            0 => workspace.resumed_from = None,
            1 => workspace.execution_seed.clear(),
            2 => workspace
                .execution_seed
                .push(workspace.execution_seed[0].clone()),
            3 => {
                workspace.execution_seed[0].occurrence.node =
                    NodeName::new("foreign").assert_value()
            }
            _ => workspace.execution_seed[0].dispatch_position = HistoryPosition::MAX,
        }
        let history = workspace.execution_seed.clone();
        let error = workspace
            .settle_request(&RunId::new("successor"))
            .unwrap_err();
        let expected = match invalid {
            0 => "request continuation requires its predecessor",
            4 => "history position",
            _ => "request checkpoint must contain its one pending execution",
        };
        assert!(error.contains(expected), "{error}");
        assert_eq!(workspace.execution_seed, history);
        assert!(workspace.vector_seed.is_empty());
    }
}

#[test]
fn explicit_expiry_settles_after_all_restored_history_and_without_replaying_it() {
    use crate::full_v1_reducer::ExecutionVoidReason;
    let mut workspace = restored_workspace();
    let mut earlier = workspace.execution_seed[0].clone();
    earlier.occurrence.node = NodeName::new("earlier").assert_value();
    earlier.state = DurableExecutionState::Settled {
        position: HistoryPosition::new(5).assert_value(),
        outcome: openengine_cluster_protocol::RequestResolution::Expiry {}.outcome(),
    };
    let mut voided = earlier.clone();
    voided.node_instance = NodeInstanceId::new(2).assert_value();
    voided.execution = ExecutionId::new(2).assert_value();
    voided.occurrence.node = NodeName::new("loser").assert_value();
    voided.dispatch_position = HistoryPosition::new(6).assert_value();
    voided.state = DurableExecutionState::Voided {
        position: HistoryPosition::new(8).assert_value(),
        reason: ExecutionVoidReason::ParallelJoin,
    };
    workspace.execution_seed[0].node_instance = NodeInstanceId::new(3).assert_value();
    workspace.execution_seed[0].execution = ExecutionId::new(3).assert_value();
    workspace.execution_seed[0].dispatch_position = HistoryPosition::new(10).assert_value();
    workspace
        .execution_seed
        .extend([earlier.clone(), voided.clone()]);
    let successor = RunId::new("successor");
    workspace.settle_request(&successor).assert_value();
    assert_eq!(workspace.execution_seed[1..], [earlier, voided]);
    assert!(matches!(&workspace.execution_seed[0].state,
        DurableExecutionState::Settled { position, outcome }
        if position.get() == 11 && *outcome == openengine_cluster_protocol::RequestResolution::Expiry {}.outcome()));
    assert_eq!(workspace.vector_seed.len(), 1);
    assert_eq!(workspace.vector_seed[0].run_id, successor);
    assert!(matches!(&workspace.vector_seed[0].content,
        openengine_cluster_protocol::RunVectorContent::Expiry { request_id } if request_id.as_str() == "question"));
    let history = workspace.execution_seed.clone();
    workspace.settle_request(&successor).assert_value();
    assert_eq!(workspace.execution_seed, history);
    assert_eq!(
        workspace.vector_seed.len(),
        1,
        "a consumed continuation cannot settle twice"
    );
}
