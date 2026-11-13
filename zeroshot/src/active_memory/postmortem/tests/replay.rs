use super::*;
use crate::native_v2_contract::ExecutionId;
use crate::v2_run_ledger::{
    AppendResult, CreateRunOutcome, RunLedgerError, RunSummary, SnapshotAndTail, StoredRun,
    StoredRunEvent, SafeLogLine, SafeLogStream, cursor_for, cursor_sequence, MAX_REPLAY_EVENTS,
};
use openengine_cluster_protocol::{Cursor, UnixTimestampMillis};
use tokio::sync::watch;
use std::sync::atomic::AtomicBool;

struct Replay {
    snapshot: RunSnapshot,
    events: Vec<StoredRunEvent>,
    reads: AtomicUsize,
    stalled: AtomicBool,
    entered: tokio::sync::Notify,
    cancel: Option<watch::Sender<bool>>,
}

#[async_trait]
impl RunLedger for Replay {
    async fn create_or_get(&self, _: CreateRun) -> Result<CreateRunOutcome, RunLedgerError> {
        Err(RunLedgerError::Storage)
    }
    async fn get(&self, _: &RunId) -> Result<Option<StoredRun>, RunLedgerError> {
        Err(RunLedgerError::Storage)
    }
    async fn get_by_submission_key(
        &self,
        _: &IdempotencyKey,
    ) -> Result<Option<StoredRun>, RunLedgerError> {
        Err(RunLedgerError::Storage)
    }
    async fn list(&self) -> Result<Vec<RunSummary>, RunLedgerError> {
        Err(RunLedgerError::Storage)
    }
    async fn append(&self, _: &RunId, _: Vec<RunEvent>) -> Result<AppendResult, RunLedgerError> {
        Err(RunLedgerError::Storage)
    }
    async fn request_force_stop(&self, _: &RunId) -> Result<AppendResult, RunLedgerError> {
        Err(RunLedgerError::Storage)
    }
    async fn request_suspend(&self, _: &RunId) -> Result<AppendResult, RunLedgerError> {
        Err(RunLedgerError::Storage)
    }
    async fn vector_cursor(
        &self,
        _: &RunId,
        _: &openengine_cluster_protocol::VectorId,
    ) -> Result<Cursor, RunLedgerError> {
        Err(RunLedgerError::Storage)
    }
    async fn snapshot_and_tail(
        &self,
        _: &RunId,
        after: Option<&Cursor>,
    ) -> Result<SnapshotAndTail, RunLedgerError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.stalled.load(Ordering::SeqCst) {
            self.entered.notify_one();
            return std::future::pending().await;
        }
        if let Some(cancel) = &self.cancel {
            cancel.send_replace(true);
        }
        let after = after.map(cursor_sequence).transpose()?.unwrap_or_default();
        Ok(SnapshotAndTail {
            snapshot: self.snapshot.clone(),
            events: self
                .events
                .iter()
                .filter(|event| {
                    cursor_sequence(&event.cursor).is_ok_and(|sequence| sequence > after)
                })
                .take(MAX_REPLAY_EVENTS)
                .cloned()
                .collect(),
        })
    }
}

async fn replay(cancel: Option<watch::Sender<bool>>) -> (PostmortemRequest, Arc<Replay>) {
    let fixture = fixture(Default::default(), None, true).await;
    fixture.drive().await;
    let mut request = fixture.postmortem_request().await;
    let mut second_visit = request
        .snapshot
        .executions
        .values()
        .next()
        .assert_value()
        .clone();
    let first = second_visit.reference.execution;
    let second = ExecutionId::new(first.get() + 1).assert_value();
    second_visit.reference.execution = second;
    request.snapshot.executions.insert(second, second_visit);
    request.snapshot.cursor = cursor_for(6000);
    let events = (1..=6001)
        .map(|sequence| StoredRunEvent {
            cursor: cursor_for(sequence),
            event: RunEvent::SafeLog {
                execution: Some(if sequence == 1 { first } else { second }),
                timestamp: UnixTimestampMillis::new(1).assert_value(),
                stream: SafeLogStream::Output,
                line: SafeLogLine::new(if sequence == 1 {
                    "earlier worker finding"
                } else {
                    "late execution delta"
                })
                .assert_value(),
                diagnostic: None,
            },
        })
        .collect();
    let replay = Arc::new(Replay {
        snapshot: request.snapshot.clone(),
        events,
        reads: AtomicUsize::new(0),
        stalled: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        cancel,
    });
    request.ledger = replay.clone();
    (request, replay)
}

#[tokio::test]
async fn paged_replay_retains_early_worker_evidence_and_stops_at_the_frozen_cursor() {
    let (request, replay) = replay(None).await;
    let evidence = evidence::collect(&request, Vec::new()).await.assert_value();
    assert!(replay.reads.load(Ordering::SeqCst) > 1);
    assert!(evidence.references.contains("v2:1"));
    assert!(evidence.references.contains("v2:6000"));
    assert!(!evidence.references.contains("v2:6001"));
    assert!(
        evidence.document["records"]
            .as_array()
            .assert_value()
            .iter()
            .any(|record| record["event"]
                .as_str()
                .is_some_and(|text| text.contains("earlier worker finding")))
    );
    assert_eq!(evidence.document["evidence_truncated"], true);
}

#[tokio::test]
async fn cancellation_during_replay_drains_the_current_page_and_starts_no_next_read() {
    let (cancel, cancellation) = watch::channel(false);
    let (request, replay) = replay(Some(cancel)).await;
    assert_cancelled(&request, &replay, &cancellation, 1).await;
}

#[tokio::test]
async fn cancelled_or_closed_collection_never_starts_a_ledger_read() {
    for initially_cancelled in [true, false] {
        let (sender, cancellation) = watch::channel(initially_cancelled);
        let _keep_open = initially_cancelled.then_some(sender);
        let (request, replay) = replay(None).await;
        assert_cancelled(&request, &replay, &cancellation, 0).await;
    }
}

#[tokio::test]
async fn cancellation_on_the_last_page_still_discards_the_collected_evidence() {
    let (cancel, cancellation) = watch::channel(false);
    let (mut request, replay) = replay(Some(cancel)).await;
    request.snapshot.cursor = cursor_for(1);
    assert_cancelled(&request, &replay, &cancellation, 1).await;
}

async fn assert_cancelled(
    request: &PostmortemRequest,
    replay: &Replay,
    cancellation: &watch::Receiver<bool>,
    expected_reads: usize,
) {
    assert!(
        evidence::collect_cancellable(request, Vec::new(), cancellation)
            .await
            .assert_value()
            .is_none()
    );
    assert_eq!(replay.reads.load(Ordering::SeqCst), expected_reads);
}

#[tokio::test]
async fn a_stalled_preparation_ledger_is_dropped_at_deadline_or_explicit_cancellation() {
    for hard_stop in [false, true] {
        let (request, replay) = replay(None).await;
        replay.stalled.store(true, Ordering::SeqCst);
        let (stop, stopping) = watch::channel(false);
        let read = super::super::read_before_deadline(
            evidence::collect_cancellable(&request, Vec::new(), &stopping),
            &stopping,
            tokio::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        tokio::pin!(read);
        tokio::select! {
            () = replay.entered.notified() => {},
            _ = &mut read => panic!("the fixture must first stall inside a ledger read"),
        }
        if hard_stop {
            stop.send_replace(true);
        } else {
            tokio::time::pause();
            tokio::time::advance(std::time::Duration::from_millis(1001)).await;
            tokio::time::resume();
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), read)
            .await
            .assert_value();
        assert!(matches!(result, Err(PostmortemError::Cancelled)) == hard_stop);
        assert!(matches!(result, Err(PostmortemError::TimedOut)) != hard_stop);
        assert_eq!(replay.reads.load(Ordering::SeqCst), 1);
    }
}
