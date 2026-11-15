use super::*;
use crate::full_v1_reducer::{ExecutionId, NodeInstanceId, StructuralOccurrence};
use crate::native_v2_contract::ExecutionRef;
use crate::v2_run_ledger::{RunSnapshot, cursor_for};
use openengine_cluster_protocol::{IdempotencyKey, PositiveInteger, Sha256Digest};

#[test]
fn an_authentic_start_completes_the_queue_record_instead_of_stopping_it() {
    let admitted = crate::native_v2_runner::test_support::admitted();
    let run_id = RunId::new("queue-test");
    let snapshot = RunSnapshot::admitted(run_id.clone(), &admitted).unwrap();
    let mut replay = ControlReplay::new(StoredRun {
        submission_key: IdempotencyKey::new("queue-test").unwrap(),
        submission_digest: Sha256Digest::new("0".repeat(64)).unwrap(),
        admitted,
        snapshot,
    });
    replay
        .apply(StoredRunEvent {
            cursor: cursor_for(1),
            event: RunEvent::RunStarted,
        })
        .unwrap();
    assert!(
        replay
            .current
            .values()
            .any(|record| record.node == "left" && record.state == "queued")
    );
    replay
        .apply(StoredRunEvent {
            cursor: cursor_for(2),
            event: RunEvent::NodeStarted {
                reference: ExecutionRef {
                    run_id,
                    node: "left".parse().unwrap(),
                    node_instance: NodeInstanceId::new(1).unwrap(),
                    execution: ExecutionId::new(1).unwrap(),
                },
                occurrence: StructuralOccurrence {
                    node: "left".parse().unwrap(),
                    map_indices: vec![],
                },
                attempt: PositiveInteger::new(1).unwrap(),
                input: Value::Null,
            },
        })
        .unwrap();
    assert!(
        replay
            .current
            .values()
            .any(|record| record.node == "left" && record.state == "completed")
    );
    assert!(
        !replay
            .updates
            .iter()
            .any(|(_, record)| record.node == "left" && record.state == "stopped")
    );
}
