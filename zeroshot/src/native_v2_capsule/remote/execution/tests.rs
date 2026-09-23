use super::*;

#[test]
fn cleanup_failure_survives_output_bridge_failure() {
    assert_eq!(
        prefer_pending_bridge_failure(
            Err(NodeRunnerError::CleanupUnconfirmed),
            Some(NodeRunnerError::DurableOutputClosed),
        ),
        Err(NodeRunnerError::CleanupUnconfirmed)
    );
}
