//! Cross-transport equivalence: the NDJSON stdio binding from #745 must reproduce the exact same
//! watch transcript (cursor progression and event algebra) as the in-process `Dispatcher::watch`
//! passthrough from #647, while sharing its connection with ordinary unary traffic and honoring
//! `subscription/cancel`. Reuses the same `CARGO_BIN_EXE_openengine-cluster-stdio` subprocess and
//! two-instance comparison pattern as `protocol_v1.rs`'s
//! `admission_transcript_matches_in_process_and_stdio`.

use std::sync::Arc;
use std::time::Duration;

use openengine_cluster_client::{
    ClusterClient, EventOrClosed, InProcessTransport, NdjsonReconnectingEventStream,
    NdjsonTransport, NdjsonWatchClient, WatchClient,
};
use openengine_cluster_protocol::{Cursor, GetParams, GraphSpec, StopMode, WatchEvent, WatchParams};
use openengine_cluster_server::admission::AdmissionCoordinator;
use openengine_cluster_server::watch::PublicEventRecord;
use openengine_cluster_server::{ConnectionContext, Dispatcher};
use openengine_cluster_testkit::admission::{
    compiled_from_graph_fixture, graph_fixture, InMemoryAdmissionStore, ScriptedOutcome,
    ScriptedVerifier,
};
use openengine_cluster_testkit::lifecycle::stop;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};

#[path = "admission_support/committed.rs"]
mod committed_support;
use committed_support::committed;

#[path = "stdio_subprocess_support/mod.rs"]
mod stdio_subprocess_support;

/// Collects [`EventOrClosed`]s from `stream` until (and including) `Finished`, panicking if the
/// stream closes first.
macro_rules! collect_transcript {
    ($stream:expr) => {{
        let mut events = Vec::new();
        loop {
            match $stream.next().await.expect("stream ended before Finished") {
                EventOrClosed::Event(record) => {
                    let finished = matches!(record.event, WatchEvent::Finished { .. });
                    events.push(record);
                    if finished {
                        break;
                    }
                }
                EventOrClosed::Closed { reason, .. } => {
                    panic!("stream closed ({reason:?}) before the Finished event was observed")
                }
            }
        }
        events
    }};
}

/// Runs one apply/get/stop lifecycle against a fresh in-process backend while a `watch`
/// subscription streams on the same dispatcher, returning its collected transcript.
async fn in_process_side_transcript(graph: &GraphSpec) -> Vec<PublicEventRecord> {
    let compiled = compiled_from_graph_fixture(graph);
    let verifier = Arc::new(ScriptedVerifier::new(vec![ScriptedOutcome::approve(
        compiled,
        vec![],
    )]));
    let store = Arc::new(InMemoryAdmissionStore::default());
    let backend = AdmissionCoordinator::from_shared(verifier, store);
    let dispatcher = Dispatcher::new(backend, ConnectionContext::default());
    let in_process_client = ClusterClient::new(InProcessTransport::new(dispatcher.clone()));
    in_process_client.initialize().await.unwrap();
    let in_process_watch = WatchClient::new(dispatcher);

    let (_parked, mut in_process_stream, _handle) = in_process_watch
        .watch(WatchParams::default())
        .await
        .unwrap();

    let apply_result = in_process_client
        .apply(committed(
            graph.clone(),
            Value::Null,
            0,
            "in-process-create",
        ))
        .await
        .unwrap();
    let generation = apply_result.generation.unwrap().get();
    // AC: a unary request completes correctly while the watch subscription is actively
    // streaming on the same connection.
    let get_result = in_process_client.get(GetParams::default()).await.unwrap();
    assert_eq!(get_result.spec, Some(graph.clone()));
    in_process_client
        .stop(stop(StopMode::Drain, generation, "in-process-stop"))
        .await
        .unwrap();
    collect_transcript!(in_process_stream)
}

fn assert_transcripts_match(in_process: &[PublicEventRecord], ndjson: &[PublicEventRecord]) {
    assert_eq!(in_process.len(), ndjson.len());
    for (in_process, ndjson) in in_process.iter().zip(ndjson.iter()) {
        assert_eq!(in_process.cursor, ndjson.cursor);
        assert_eq!(in_process.event, ndjson.event);
    }
}

/// Asserts the at-most-one-post-cancel-leak model for a subscription cancelled before its run was
/// ever committed to: the server-side subscription task may already have been parked awaiting the
/// next live event at the moment cancellation was processed, so at most one further event (the
/// commit's own first event, immediately following cancellation) may still leak through before it
/// observes cancellation on its next poll and stops for good.
async fn assert_cancel_probe_leak_model<'a, R, W>(
    mut cancel_probe: NdjsonReconnectingEventStream<'a, R, W>,
    first_committed_cursor: &Cursor,
) where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut leaked = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_millis(300), cancel_probe.next()).await {
            Ok(Some(EventOrClosed::Event(record))) => leaked.push(record),
            Ok(Some(other)) => panic!("unexpected notification after cancel: {other:?}"),
            Ok(None) | Err(_) => break,
        }
    }
    assert!(
        leaked.len() <= 1,
        "cancelled probe subscription received more than one post-cancel event: {leaked:?}"
    );
    if let Some(record) = leaked.first() {
        assert_eq!(
            record.cursor, *first_committed_cursor,
            "cancellation failed to stop delivery before the run's first committed event"
        );
    }
}

#[tokio::test]
async fn ndjson_watch_transcript_matches_in_process_and_shares_its_connection() {
    let graph = graph_fixture("worker", serde_json::json!({"kind":"null"}));

    let in_process_events = in_process_side_transcript(&graph).await;

    // NDJSON side, against a fresh subprocess wired the same way (see
    // `openengine-cluster-testkit/src/bin/openengine-cluster-stdio.rs`).
    let (subprocess, stdin, stdout) = stdio_subprocess_support::spawn();
    let transport = NdjsonTransport::new(stdout, stdin);
    let ndjson_client = ClusterClient::new(&transport);
    ndjson_client.initialize().await.unwrap();
    let ndjson_watch = NdjsonWatchClient::new(&transport);

    let (_parked, mut ndjson_stream) = ndjson_watch.watch(WatchParams::default()).await.unwrap();

    // AC: `subscription/cancel` releases only the cancelled subscription. A second, still-parked
    // subscription is cancelled immediately; it must observe nothing further even though it would
    // otherwise park-attach to the very run committed below.
    let (_parked, cancel_probe) = ndjson_watch.watch(WatchParams::default()).await.unwrap();
    cancel_probe.cancel().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let apply_result = ndjson_client
        .apply(committed(
            graph.clone(),
            Value::Null,
            0,
            "ndjson-wire-create",
        ))
        .await
        .unwrap();
    let generation = apply_result.generation.unwrap().get();
    // AC: a unary request completes correctly while the watch subscription is actively
    // streaming on the same connection.
    let get_result = ndjson_client.get(GetParams::default()).await.unwrap();
    assert_eq!(get_result.spec, Some(graph.clone()));
    ndjson_client
        .stop(stop(StopMode::Drain, generation, "ndjson-wire-stop"))
        .await
        .unwrap();
    let ndjson_events = collect_transcript!(ndjson_stream);

    assert_transcripts_match(&in_process_events, &ndjson_events);
    assert_cancel_probe_leak_model(cancel_probe, &ndjson_events[0].cursor).await;

    drop(ndjson_stream);
    drop(transport);
    subprocess.join().await;
}
