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
