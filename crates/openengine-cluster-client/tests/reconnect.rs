//! Client-side watch dedup and reconnect: `WatchClient`/`ReconnectingEventStream` must recover
//! from a `SLOW_CONSUMER` close by reconnecting from the last delivered cursor with zero gap,
//! and must silently drop legal at-least-once physical duplicates, against a minimal fixture
//! store independent of the testkit's `InMemoryAdmissionStore`.

use std::sync::Arc;

use openengine_cluster_client::{EventOrClosed, WatchClient};
use openengine_cluster_protocol::{
    ClusterStatus, Cursor, RunId, StopMode, SubscriptionCloseReason, WatchEvent, WatchParams,
};
use openengine_cluster_server::watch::fixtures::{FixtureBackend, FixtureStore};
use openengine_cluster_server::{ConnectionContext, Dispatcher};

/// The fixture's queue is bounded to 2 entries (fixed at construction, independent of whatever
/// capacity `Dispatcher::watch`'s hardcoded default would otherwise request); the third publish
/// overflows it, forcing a deterministic `SLOW_CONSUMER` close for this test to reconnect from.
const FIXTURE_QUEUE_CAPACITY: usize = 2;

#[tokio::test]
async fn reconnect_after_slow_consumer_recovers_with_no_gap_and_dedups_duplicates() {
    let run_id = RunId::new("run-1");
    let store = Arc::new(FixtureStore::new(
        run_id.clone(),
        Vec::new(),
        FIXTURE_QUEUE_CAPACITY,
    ));
    let dispatcher = Dispatcher::new(
        FixtureBackend {
            store: Arc::clone(&store),
        },
        ConnectionContext::default(),
    );
    let client = WatchClient::new(dispatcher);

    let (result, mut stream, _handle) = client.watch(WatchParams::default()).await.unwrap();
    assert_eq!(result.run_id, Some(run_id));

    // The fixture's queue holds two entries; the third overflows it.
    store.publish(WatchEvent::Bookmark).await;
    store.publish(WatchEvent::Bookmark).await;
    store.publish(WatchEvent::Bookmark).await;

    let mut received = Vec::new();
    loop {
        match stream.next().await.unwrap() {
            EventOrClosed::Event(record) => received.push(record.cursor.clone()),
            EventOrClosed::Closed {
                reason,
                last_delivered_cursor,
            } => {
                assert_eq!(reason, SubscriptionCloseReason::SlowConsumer);
                assert_eq!(last_delivered_cursor, Some(Cursor::new("cursor-2")));
                break;
            }
        }
    }
    assert_eq!(received, [Cursor::new("cursor-1"), Cursor::new("cursor-2")]);

    let (_result, mut stream, _handle) = client.reconnect(stream).await.unwrap();

    // The reconnect must replay cursor-3 (recorded despite the overflow) with no gap.
    let Some(EventOrClosed::Event(record)) = stream.next().await else {
        panic!("expected the reconnect to replay the un-delivered cursor-3 event");
    };
    assert_eq!(record.cursor, Cursor::new("cursor-3"));
    received.push(record.cursor);

    // A legal at-least-once physical duplicate of the last event must be silently dropped, and
    // exactly one Finished event must be observed.
    store.republish_last().await;
    store
        .publish(WatchEvent::Finished {
            final_status: ClusterStatus::empty(),
            stop_mode: Some(StopMode::Drain),
        })
        .await;

    let mut finished_count = 0;
    while finished_count == 0 {
        match stream.next().await.unwrap() {
            EventOrClosed::Event(record) => {
                assert!(
                    !received.contains(&record.cursor),
                    "duplicate cursor delivered to the client: {:?}",
                    record.cursor
                );
                if matches!(record.event, WatchEvent::Finished { .. }) {
                    finished_count += 1;
                }
                received.push(record.cursor);
            }
            EventOrClosed::Closed { .. } => {
                panic!("stream closed before the Finished event was observed")
            }
        }
    }
    assert_eq!(finished_count, 1);
    assert_eq!(received, store.history_cursors().await);
}
