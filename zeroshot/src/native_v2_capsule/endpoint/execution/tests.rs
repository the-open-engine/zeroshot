use openengine_cluster_protocol::{MAX_SAFE_GENERATION, NodeName, RunId, TokenCount};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

fn usage(tokens: u64) -> TokenUsageDelta {
    TokenUsageDelta {
        input_tokens: TokenCount::new(tokens).assert_value(),
        output_tokens: TokenCount::new(tokens).assert_value(),
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
    }
}

#[test]
fn boundary_contract_usage_overflow_is_incomplete_and_overrides_cancellation() {
    let mut metadata = TerminalMetadata::default();
    metadata.retain(CapsuleNodeEvent::TokenUsage {
        usage: Some(usage(MAX_SAFE_GENERATION)),
    });
    metadata.retain(CapsuleNodeEvent::TokenUsage {
        usage: Some(usage(1)),
    });

    let events = metadata.into_events(Err(NodeRunnerError::Cancelled));
    assert!(matches!(
        events.as_slice(),
        [
            CapsuleNodeEvent::TokenUsage { usage: Some(known) },
            CapsuleNodeEvent::TokenUsage { usage: None },
            CapsuleNodeEvent::Failed {
                failure: CapsuleNodeFailure::ExecutionFailed
            }
        ] if known.input_tokens.get() == MAX_SAFE_GENERATION
    ));
}

#[test]
fn boundary_contract_cleanup_failure_survives_usage_overflow_and_wire_roundtrip() {
    let metadata = TerminalMetadata {
        overflowed: true,
        ..TerminalMetadata::default()
    };
    let events = metadata.into_events(Err(NodeRunnerError::CleanupUnconfirmed));
    let encoded = serde_json::to_string(&events).assert_value();
    let decoded: Vec<CapsuleNodeEvent> = serde_json::from_str(&encoded).assert_value();
    let [CapsuleNodeEvent::Failed { failure }] = decoded.as_slice() else {
        panic!("cleanup failure must survive metadata and serialization");
    };
    assert_eq!(failure.into_runner(), NodeRunnerError::CleanupUnconfirmed);
}

#[test]
fn boundary_contract_usage_reduction_handles_incomplete_optional_and_each_overflow_boundary() {
    let mut metadata = TerminalMetadata::default();
    metadata.retain(CapsuleNodeEvent::Output {
        output: CapsuleOutput {
            stream: CapsuleOutputStream::System,
            text: "ignored".to_owned(),
        },
        timestamp: UnixTimestampMillis::new(1).assert_value(),
    });
    metadata.retain(CapsuleNodeEvent::TokenUsage { usage: None });
    metadata.retain(CapsuleNodeEvent::TokenUsage {
        usage: Some(usage(2)),
    });
    metadata.retain(CapsuleNodeEvent::TokenUsage {
        usage: Some(usage(3)),
    });
    assert_eq!(metadata.known_usage.assert_value().input_tokens.get(), 5);
    assert!(metadata.incomplete);

    let maximum = TokenCount::new(MAX_SAFE_GENERATION).assert_value();
    let one = TokenCount::new(1).assert_value();
    let base = TokenUsageDelta {
        input_tokens: one,
        output_tokens: maximum,
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
    };
    assert!(add_usage(base, usage(1)).is_none());

    let cache_read = TokenUsageDelta {
        input_tokens: one,
        output_tokens: one,
        cache_read_input_tokens: Some(maximum),
        cache_creation_input_tokens: None,
    };
    let cache_read_increment = TokenUsageDelta {
        cache_read_input_tokens: Some(one),
        ..usage(1)
    };
    assert!(add_usage(cache_read, cache_read_increment).is_none());

    let cache_create = TokenUsageDelta {
        input_tokens: one,
        output_tokens: one,
        cache_read_input_tokens: None,
        cache_creation_input_tokens: Some(maximum),
    };
    let cache_create_increment = TokenUsageDelta {
        cache_creation_input_tokens: Some(one),
        ..usage(1)
    };
    assert!(add_usage(cache_create, cache_create_increment).is_none());
    assert_eq!(add_optional_usage(Some(one), None), Some(None));
}

#[tokio::test]
async fn boundary_contract_completion_and_reservation_helpers_fail_closed_without_touching_other_entries()
 {
    let (terminal, receiver) = oneshot::channel();
    send_local_completion(terminal, None, false, TerminalMetadata::default());
    receiver.await.assert_error();

    let (terminal, receiver) = oneshot::channel();
    send_local_completion(
        terminal,
        Some(Err(NodeRunnerError::Cancelled)),
        true,
        TerminalMetadata::default(),
    );
    receiver.await.assert_error();

    let (terminal, receiver) = oneshot::channel();
    send_local_completion(
        terminal,
        Some(Err(NodeRunnerError::SessionLost)),
        false,
        TerminalMetadata::default(),
    );
    assert!(matches!(
        receiver.await.assert_value().as_slice(),
        [CapsuleNodeEvent::Failed {
            failure: CapsuleNodeFailure::SessionLost
        }]
    ));

    let reference = reference();
    let (first_done, first_receiver) = watch::channel(false);
    let (second_done, second_receiver) = watch::channel(false);
    let (first_cancel, _) = watch::channel(false);
    let (second_cancel, _) = watch::channel(false);
    let state = Mutex::new(EndpointState {
        active: vec![
            EndpointExecution {
                reference: reference.clone(),
                cancel: first_cancel,
                done: first_receiver,
            },
            EndpointExecution {
                reference: reference.clone(),
                cancel: second_cancel,
                done: second_receiver,
            },
        ],
        ..EndpointState::default()
    });
    remove_endpoint_execution(&state, &reference, &first_done).await;
    let state_guard = state.lock().await;
    assert_eq!(state_guard.active.len(), 1);
    assert!(
        state_guard.active[0]
            .done
            .same_channel(&second_done.subscribe())
    );
    drop(state_guard);
    remove_endpoint_execution(&state, &reference, &first_done).await;
    assert_eq!(state.lock().await.active.len(), 1);
}

fn reference() -> ExecutionRef {
    ExecutionRef {
        run_id: RunId::new("run-endpoint-reducer"),
        node: NodeName::new("worker").assert_value(),
        node_instance: crate::native_v2_contract::NodeInstanceId::new(1).assert_value(),
        execution: crate::native_v2_contract::ExecutionId::new(1).assert_value(),
    }
}
