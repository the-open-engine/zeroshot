use super::*;
use crate::native_v2_contract::ExecutionId;
use crate::v2_run_ledger::{SafeLogLine, SafeLogStream};
use openengine_cluster_protocol::UnixTimestampMillis;
use openengine_cluster_testkit::assertions::AssertValue;

fn record(sequence: u64, execution: ExecutionId, line: &str) -> StoredRunEvent {
    StoredRunEvent {
        cursor: cursor_for(sequence),
        event: RunEvent::SafeLog {
            execution: Some(execution),
            timestamp: UnixTimestampMillis::new(1).assert_value(),
            stream: SafeLogStream::Output,
            line: SafeLogLine::new(line).assert_value(),
            diagnostic: None,
        },
    }
}

#[test]
fn a_late_reviewer_cannot_evict_quiet_worker_evidence() {
    let worker = ExecutionId::new(1).assert_value();
    let reviewer = ExecutionId::new(2).assert_value();
    let mut tail = EvidenceTail::default();
    tail.push(
        worker,
        record(
            1,
            worker,
            "worker experiment established a recurring failure cause",
        ),
    )
    .assert_value();
    for sequence in 2..6000 {
        tail.push(reviewer, record(sequence, reviewer, "review delta"))
            .assert_value();
    }
    let retained = tail.records();
    assert!(
        retained.iter().any(
            |(reference, text)| reference == "v2:1" && text.contains("recurring failure cause")
        )
    );
    assert_eq!(retained.last().assert_value().0, "v2:5999");
    assert!(tail.truncated);
    assert!(tail.bytes <= LOG_BUDGET);
    assert_eq!(
        tail.bytes,
        retained.iter().map(|(_, text)| text.len()).sum::<usize>()
    );
}

#[test]
fn interleaved_execution_tails_preserve_real_cursor_order_and_utf8_bounds() {
    let mut tail = EvidenceTail::default();
    for sequence in 1..2000 {
        let execution = ExecutionId::new(sequence % 3 + 1).assert_value();
        tail.push(execution, record(sequence, execution, &"é".repeat(900)))
            .assert_value();
    }
    let retained = tail.records();
    let sequences = retained
        .iter()
        .map(|(reference, _)| reference[3..].parse::<u64>().assert_value())
        .collect::<Vec<_>>();
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        tail.executions
            .values()
            .all(|execution| !execution.records.is_empty())
    );
    assert_eq!(tail.executions.len(), 3);
    assert!(tail.truncated && tail.bytes <= LOG_BUDGET);
    assert!(retained.iter().all(|(_, text)| text.len() <= 1024));
}
