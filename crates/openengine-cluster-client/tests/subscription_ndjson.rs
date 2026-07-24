//! Client-side NDJSON watch dedup and reconnect: `NdjsonWatchClient`/
//! `NdjsonReconnectingEventStream` must recover from a `SLOW_CONSUMER` close by reconnecting from
//! the last delivered cursor with zero gap, and must silently drop legal at-least-once physical
//! duplicates, driven over the wire against `serve_ndjson` instead of the in-process
//! `Dispatcher::watch` passthrough exercised by `tests/reconnect.rs`.

use std::sync::Arc;
use std::time::Duration;

use openengine_cluster_client::{ClusterClient, EventOrClosed, NdjsonTransport, NdjsonWatchClient};
use openengine_cluster_protocol::{Cursor, GetParams, RunId, WatchEvent, WatchParams};
use openengine_cluster_server::watch::fixtures::{
    await_ndjson_shutdown, spawn_ndjson, FixtureBackend, FixtureStore,
};

#[path = "reconnect_support/mod.rs"]
mod reconnect_support;
use reconnect_support::{
    assert_reconnect_replays_and_dedups, overflow_and_close, FIXTURE_QUEUE_CAPACITY,
};

#[tokio::test]
async fn reconnect_after_slow_consumer_recovers_with_no_gap_and_dedups_duplicates_over_ndjson() {
    let run_id = RunId::new("run-1");
    let store = Arc::new(FixtureStore::new(
        run_id.clone(),
        Vec::new(),
        FIXTURE_QUEUE_CAPACITY,
    ));
    let (client_write, client_read, server) = spawn_ndjson(FixtureBackend::new(Arc::clone(&store)));

    let transport = NdjsonTransport::new(client_read, client_write);
    let watch_client = NdjsonWatchClient::new(&transport);

    let (result, mut stream) = watch_client.watch(WatchParams::default()).await.unwrap();
    assert_eq!(result.run_id, Some(run_id));

    let received = overflow_and_close(&store, &mut stream).await;

    let (_result, mut stream) = stream.reconnect().await.unwrap();
    assert_reconnect_replays_and_dedups(&store, &mut stream, received).await;

    drop(stream);
    drop(transport);
    await_ndjson_shutdown(server).await;
}

#[tokio::test]
async fn cancel_stops_further_delivery() {
    let run_id = RunId::new("run-1");
    let store = Arc::new(FixtureStore::new(run_id, Vec::new(), 8));
    let (client_write, client_read, server) = spawn_ndjson(FixtureBackend::new(Arc::clone(&store)));

    let transport = NdjsonTransport::new(client_read, client_write);
    let watch_client = NdjsonWatchClient::new(&transport);
    let (_result, mut stream) = watch_client.watch(WatchParams::default()).await.unwrap();

    store.publish(WatchEvent::Bookmark).await;
    match stream.next().await.unwrap() {
        EventOrClosed::Event(record) => assert_eq!(record.cursor, Cursor::new("cursor-1")),
        other => panic!("expected an event, got {other:?}"),
    }

    stream.cancel().await.unwrap();

    // `cancel()` only writes a fire-and-forget notification line -- it does not wait for the
    // server to have applied it. Force a synchronous round trip on the same connection so the
    // subsequent publishes are guaranteed to happen only after the server's read loop has already
    // processed (and synchronously applied) the preceding cancel line; NDJSON lines are read and
    // handled strictly in order, so a response to a request sent after `cancel` can only arrive
    // once the cancel itself has already been read and applied.
    ClusterClient::new(&transport)
        .get(GetParams::default())
        .await
        .unwrap();

    // The server-side subscription task may already be parked awaiting the next live event at
    // the moment cancellation is processed, so at most one further event (the one immediately
    // following cancellation) may still be delivered before it observes cancellation and stops
    // for good; no `subscription/closed` follows a plain cancel, so absence of further delivery
    // is observed as this stream's `next()` simply never resolving again.
    store.publish(WatchEvent::Bookmark).await; // may leak as cursor-2
    store.publish(WatchEvent::Bookmark).await; // cursor-3, must never arrive

    let mut leaked = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_millis(300), stream.next()).await {
            Ok(Some(EventOrClosed::Event(record))) => leaked.push(record.cursor),
            Ok(Some(other)) => panic!("unexpected notification after cancel: {other:?}"),
            Ok(None) | Err(_) => break,
        }
    }
    assert!(
        leaked.len() <= 1,
        "cancelled subscription received more than one post-cancel event: {leaked:?}"
    );
    if let Some(cursor) = leaked.first() {
        assert_eq!(
            *cursor,
            Cursor::new("cursor-2"),
            "cancellation failed to stop delivery before the next published event"
        );
    }

    drop(stream);
    drop(transport);
    await_ndjson_shutdown(server).await;
}
