//! NDJSON stdio transport: multiplexes unary JSON-RPC request/response traffic and generic
//! `watch` subscription notifications over one bounded-frame connection.

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use openengine_cluster_protocol::{
    DomainErrorData, EventNotification, JsonRpcNotification, JsonRpcRequest, RequestId,
    SubscriptionCancelParams, SubscriptionClosedNotification, SubscriptionId, WatchParams,
    INVALID_PARAMS, INVALID_REQUEST, JSON_RPC_VERSION, PARSE_ERROR, SCHEMA_VIOLATION,
};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinSet;
use tokio_stream::StreamExt;
use tokio_util::codec::{Framed, LinesCodec, LinesCodecError};

use crate::watch::{WatchEventStream, WatchHandle, WatchStreamItem};
use crate::{serialize_backend_error, serialize_error, serialize_success, ClusterBackend, Dispatcher};

/// Bounded NDJSON frame length. A line exceeding this (with no terminating newline found first)
/// is rejected with a `PARSE_ERROR` frame rather than buffered without limit.
const MAX_FRAME_BYTES: usize = 1_048_576;

/// Bounded per-connection outbound queue: unary responses and subscription notifications share
/// this single writer queue, so one pathologically slow peer backpressures further writes rather
/// than growing memory without bound.
const OUTBOUND_QUEUE_CAPACITY: usize = 256;

/// Grace period given to already-spawned tasks to finish on their own once the connection is
/// shutting down (EOF or fatal I/O error). Bounded unary/watch-establishment dispatches finish
/// well within this window and get their response flushed; a `watch` subscription's streaming
/// loop has no natural end once its live channel goes idle (cancellation is only observed on the
/// stream's next poll, and nothing left will ever poll it), so once the grace period elapses
/// whatever tasks remain are assumed stuck and force-aborted rather than awaited forever.
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_millis(200);

/// Domain error code for a request id reused while its first request is still in flight. Specific
/// to this multiplexed NDJSON binding: an unmultiplexed unary transport has no concept of two
/// requests in flight at once.
const DUPLICATE_REQUEST_ID: &str = "DUPLICATE_REQUEST_ID";

/// Per-subscription cancellation signal: notifying it wakes `run_watch_subscription`'s streaming
/// loop immediately, even while parked awaiting the next live event, instead of relying solely on
/// `WatchEventStream`'s own cancelled flag, which is only re-checked at the top of `next()` and so
/// never observed on an idle run once the task is parked inside `next_live`'s
/// `receiver.recv().await`.
type SubscriptionMap = Arc<StdMutex<HashMap<SubscriptionId, Arc<Notify>>>>;
type InFlightIds = Arc<StdMutex<HashSet<RequestId>>>;

/// Per-connection state shared by every spawned request/subscription task: the outbound write
/// queue and the tracking maps used for cancellation and duplicate-id rejection.
#[derive(Clone)]
struct ConnectionState {
    outbound_tx: mpsc::Sender<String>,
    subscriptions: SubscriptionMap,
    in_flight_ids: InFlightIds,
}

pub async fn serve_ndjson<B, R, W, E>(
    dispatcher: Dispatcher<B>,
    reader: R,
    writer: W,
    mut diagnostics: E,
) -> io::Result<()>
where
    B: ClusterBackend,
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
    E: AsyncWrite + Send + Unpin + 'static,
{
    let (outbound_tx, outbound_rx) = mpsc::channel::<String>(OUTBOUND_QUEUE_CAPACITY);
    let writer_task = tokio::spawn(run_writer(writer, outbound_rx));

    let subscriptions: SubscriptionMap = Arc::new(StdMutex::new(HashMap::new()));
    let in_flight_ids: InFlightIds = Arc::new(StdMutex::new(HashSet::new()));
    let mut tasks: JoinSet<()> = JoinSet::new();
    let state = ConnectionState {
        outbound_tx: outbound_tx.clone(),
        subscriptions: Arc::clone(&subscriptions),
        in_flight_ids: Arc::clone(&in_flight_ids),
    };

    let mut lines = Framed::new(reader, LinesCodec::new_with_max_length(MAX_FRAME_BYTES));
    loop {
        let line = match lines.next().await {
            Some(Ok(line)) => line,
            Some(Err(LinesCodecError::MaxLineLengthExceeded)) => {
                let _ = outbound_tx
                    .send(serialize_error(None, PARSE_ERROR, "Parse error", None))
                    .await;
                // `Framed`'s stream terminates for good after yielding any decode error (it
                // never calls `decode` again), so `LinesCodec`'s own discard-until-next-newline
                // resync would otherwise never run. Rebuilding via `from_parts`/`into_parts`
                // (rather than `Framed::new` + manually restoring the read buffer) matters: the
                // buffer's leftover bytes may already contain one or more complete lines past the
                // discarded one, and only `from_parts` marks the rebuilt reader immediately
                // readable from that carried-over buffer — reconstructing via `new` and copying
                // the buffer in by hand leaves it believing the buffer is empty, so it blocks on
                // a fresh read instead of decoding what is already buffered.
                lines = Framed::from_parts(lines.into_parts());
                continue;
            }
            Some(Err(LinesCodecError::Io(error))) => {
                diagnostics
                    .write_all(format!("cluster protocol input error: {error}\n").as_bytes())
                    .await?;
                diagnostics.flush().await?;
                break;
            }
            None => break,
        };

        match classify_ndjson_line(&line) {
            NdjsonLineKind::Cancel(subscription_id) => {
                if let Some(cancel) = subscriptions.lock().unwrap().remove(&subscription_id) {
                    cancel.notify_one();
                }
            }
            NdjsonLineKind::Watch { id, params } => {
                if reject_duplicate(&in_flight_ids, &outbound_tx, id.clone()).await {
                    continue;
                }
                tasks.spawn(run_watch_subscription(
                    dispatcher.clone(),
                    id,
                    params,
                    state.clone(),
                ));
            }
            NdjsonLineKind::Passthrough { id } => {
                if let Some(id) = id.clone() {
                    if reject_duplicate(&in_flight_ids, &outbound_tx, id).await {
                        continue;
                    }
                }
                tasks.spawn(run_passthrough_request(
                    dispatcher.clone(),
                    id,
                    line,
                    state.clone(),
                ));
            }
        }
    }

    let drain_naturally = async { while tasks.join_next().await.is_some() {} };
    if tokio::time::timeout(SHUTDOWN_GRACE_PERIOD, drain_naturally)
        .await
        .is_err()
    {
        tasks.shutdown().await;
    }
    drop(subscriptions);
    drop(outbound_tx);
    drop(state);
    let _ = writer_task.await;
    Ok(())
}

/// Registers `id` as in flight, or sends a `DUPLICATE_REQUEST_ID` error and returns `true` if it
/// was already in flight.
async fn reject_duplicate(
    in_flight_ids: &InFlightIds,
    outbound_tx: &mpsc::Sender<String>,
    id: RequestId,
) -> bool {
    let inserted = in_flight_ids.lock().unwrap().insert(id.clone());
    if inserted {
        return false;
    }
    let _ = outbound_tx
        .send(serialize_error(
            Some(id),
            INVALID_REQUEST,
            "Invalid Request",
            Some(DomainErrorData::new(DUPLICATE_REQUEST_ID)),
        ))
        .await;
    true
}

/// Drains the bounded outbound queue, writing one NDJSON line per queued frame. Stops on the
/// first write failure (the peer is gone) or once every sender half is dropped.
async fn run_writer<W>(mut writer: W, mut outbound_rx: mpsc::Receiver<String>)
where
    W: AsyncWrite + Unpin,
{
    while let Some(line) = outbound_rx.recv().await {
        if writer.write_all(line.as_bytes()).await.is_err() {
            break;
        }
        if writer.write_all(b"\n").await.is_err() {
            break;
        }
        if writer.flush().await.is_err() {
            break;
        }
    }
}

/// Dispatches a non-`watch` request or notification line, releasing its in-flight id (if any)
/// once the backend call returns and before the response is enqueued.
async fn run_passthrough_request<B>(
    dispatcher: Dispatcher<B>,
    id: Option<RequestId>,
    line: String,
    state: ConnectionState,
) where
    B: ClusterBackend,
{
    let response = dispatcher.dispatch(&line).await;
    if let Some(id) = id {
        state.in_flight_ids.lock().unwrap().remove(&id);
    }
    let _ = state.outbound_tx.send(response).await;
}

/// Establishes a `watch` subscription and, on success, streams its `event`/`subscription/closed`
/// notifications until the stream ends (overflow, backend close, or cancellation). Registers a
/// per-subscription cancellation [`Notify`] in `subscriptions` for the duration so a concurrent
/// `subscription/cancel` wakes the streaming loop immediately -- even while parked awaiting the
/// next live event -- instead of only being observed the next time `WatchEventStream::next()` is
/// polled, which never happens again on an idle run. The established [`WatchHandle`] is kept alive
/// for the same duration purely to hold its backing flag false: dropping it early would trip
/// `WatchEventStream`'s own cancellation check before anything ever streams. Deregisters once the
/// stream stops.
async fn run_watch_subscription<B>(
    dispatcher: Dispatcher<B>,
    id: RequestId,
    params: Value,
    state: ConnectionState,
) where
    B: ClusterBackend,
{
    let ConnectionState {
        outbound_tx,
        subscriptions,
        in_flight_ids,
    } = state;
    let (response, established) = dispatcher.dispatch_watch(id.clone(), params).await;
    in_flight_ids.lock().unwrap().remove(&id);
    let Some((subscription_id, mut stream, _handle)) = established else {
        let _ = outbound_tx.send(response).await;
        return;
    };
    let cancel = Arc::new(Notify::new());
    // Register before sending the response: `mpsc::Sender::send` can yield under backpressure
    // or Tokio's cooperative-scheduling budget, and a `subscription/cancel` racing in during that
    // yield must always find the subscription already cancellable (mirrors the client's
    // register-before-resolve ordering in `NdjsonTransport::run_pump`).
    subscriptions
        .lock()
        .unwrap()
        .insert(subscription_id.clone(), Arc::clone(&cancel));
    if outbound_tx.send(response).await.is_err() {
        subscriptions.lock().unwrap().remove(&subscription_id);
        return;
    }

    loop {
        // Race the stream against `cancel` so a `subscription/cancel` that arrives while this task
        // is parked inside `next_live`'s `receiver.recv().await` (the steady state for an idle
        // subscription) wakes it right away instead of leaking the task and its channel for the
        // rest of the connection's lifetime. `biased` checks `cancel` first: without it, once a
        // cancellation is pending, `select!`'s default random tie-break could still occasionally
        // favor draining more already-buffered stream items over observing the cancellation,
        // letting an unbounded number of already-buffered events leak instead of at most the one
        // that may already be in flight.
        let item = tokio::select! {
            biased;
            () = cancel.notified() => None,
            item = stream.next() => item,
        };
        let Some(item) = item else {
            break;
        };
        let notification = match item {
            WatchStreamItem::Record(record) => serde_json::to_string(&JsonRpcNotification {
                jsonrpc: JSON_RPC_VERSION.to_owned(),
                method: "event".to_owned(),
                params: EventNotification {
                    subscription_id: subscription_id.clone(),
                    run_id: record.run_id,
                    cursor: record.cursor,
                    event: record.event,
                },
            })
            .expect("event notification serialization must succeed"),
            WatchStreamItem::Closed {
                reason,
                last_delivered_cursor,
            } => serde_json::to_string(&JsonRpcNotification {
                jsonrpc: JSON_RPC_VERSION.to_owned(),
                method: "subscription/closed".to_owned(),
                params: SubscriptionClosedNotification {
                    subscription_id: subscription_id.clone(),
                    reason,
                    last_delivered_cursor,
                },
            })
            .expect("subscription closed notification serialization must succeed"),
        };
        if outbound_tx.send(notification).await.is_err() {
            break;
        }
    }
    subscriptions.lock().unwrap().remove(&subscription_id);
}

pub async fn serve_stdio<B>(dispatcher: Dispatcher<B>) -> io::Result<()>
where
    B: ClusterBackend,
{
    serve_ndjson(
        dispatcher,
        tokio::io::stdin(),
        tokio::io::stdout(),
        tokio::io::stderr(),
    )
    .await
}

impl<B> Dispatcher<B>
where
    B: ClusterBackend,
{
    /// NDJSON-only counterpart to [`Dispatcher::dispatch`] for the `watch` method: returns the
    /// response frame plus, on success, the minted subscription identity and stream/handle to
    /// register for event fan-out. Never called from [`Dispatcher::dispatch`] since `watch` is a
    /// subscription establishment method, not a plain unary one.
    pub(crate) async fn dispatch_watch(
        &self,
        id: RequestId,
        params: Value,
    ) -> (
        String,
        Option<(SubscriptionId, WatchEventStream, WatchHandle)>,
    ) {
        let params = match serde_json::from_value::<WatchParams>(params) {
            Ok(params) => params,
            Err(_) => {
                return (
                    serialize_error(
                        Some(id),
                        INVALID_PARAMS,
                        "Invalid params",
                        Some(DomainErrorData::new(SCHEMA_VIOLATION)),
                    ),
                    None,
                );
            }
        };
        match self.watch(params).await {
            Ok((result, stream, handle)) => {
                let subscription_id = result.subscription_id.clone();
                (
                    serialize_success(id, result),
                    Some((subscription_id, stream, handle)),
                )
            }
            Err(error) => (serialize_backend_error(id, error), None),
        }
    }
}

/// Result of classifying one decoded NDJSON line for [`serve_ndjson`]'s multiplexer. `Passthrough`
/// carries the request id when the line parsed as a well-formed non-`watch` request, so the
/// multiplexer can still apply duplicate-in-flight-id detection to ordinary unary methods; it is
/// `None` for malformed lines or notifications, which [`Dispatcher::dispatch`] handles on its own.
pub(crate) enum NdjsonLineKind {
    Watch { id: RequestId, params: Value },
    Cancel(SubscriptionId),
    Passthrough { id: Option<RequestId> },
}

/// Classifies a decoded NDJSON line without fully deserializing its params: a `watch` request is
/// pulled out for subscription handling, a `subscription/cancel` notification is pulled out for
/// inline cancellation, and everything else (including malformed JSON) passes through to
/// [`Dispatcher::dispatch`] unchanged.
pub(crate) fn classify_ndjson_line(line: &str) -> NdjsonLineKind {
    if let Ok(request) = serde_json::from_str::<JsonRpcRequest<Value>>(line) {
        if request.method == "watch" {
            return NdjsonLineKind::Watch {
                id: request.id,
                params: request.params,
            };
        }
        return NdjsonLineKind::Passthrough {
            id: Some(request.id),
        };
    }
    if let Ok(notification) =
        serde_json::from_str::<JsonRpcNotification<SubscriptionCancelParams>>(line)
    {
        if notification.method == "subscription/cancel" {
            return NdjsonLineKind::Cancel(notification.params.subscription_id);
        }
    }
    NdjsonLineKind::Passthrough { id: None }
}

#[cfg(test)]
mod tests {
    use openengine_cluster_protocol::RunId;

    use super::*;
    use crate::watch::fixtures::{FixtureBackend, FixtureStore};
    use crate::ConnectionContext;

    /// Regression test for a race where `run_watch_subscription` sent the `watch` response before
    /// registering the subscription's `WatchHandle` in `subscriptions`: a `subscription/cancel`
    /// processed by the read loop in that window found nothing to remove and the subscription was
    /// never cancellable again. Forces the response send to block (a pre-filled, capacity-1
    /// outbound queue that nothing drains) so the task is parked exactly at that send call, then
    /// asserts registration has already happened — true only when the insert precedes the send.
    #[tokio::test]
    async fn subscription_is_registered_before_its_response_send_can_complete() {
        let store = Arc::new(FixtureStore::new(RunId::new("run-1"), Vec::new(), 8));
        let dispatcher = Dispatcher::new(FixtureBackend::new(store), ConnectionContext::default());

        let (outbound_tx, mut outbound_rx) = mpsc::channel::<String>(1);
        outbound_tx.send("occupied".to_owned()).await.unwrap();

        let subscriptions: SubscriptionMap = Arc::new(StdMutex::new(HashMap::new()));
        let state = ConnectionState {
            outbound_tx,
            subscriptions: Arc::clone(&subscriptions),
            in_flight_ids: Arc::new(StdMutex::new(HashSet::new())),
        };

        tokio::spawn(run_watch_subscription(
            dispatcher,
            RequestId::Integer(1),
            Value::Object(serde_json::Map::new()),
            state,
        ));

        // Let the spawned task run dispatch_watch to completion; it then blocks indefinitely on
        // the full outbound queue, since nothing here drains it yet. Poll via bounded cooperative
        // yields rather than a fixed sleep: a real-time sleep is a race against however long the
        // spawned task actually takes to be scheduled, which flakes under the CPU contention of a
        // full `cargo test --workspace` run; yielding is deterministic regardless of load and the
        // attempt cap still fails the test if registration never happens.
        let mut attempts = 0;
        while subscriptions.lock().unwrap().len() != 1 {
            attempts += 1;
            assert!(
                attempts < 100_000,
                "subscription was never registered before its response send could complete, \
                 so a cancel racing the response would be lost"
            );
            tokio::task::yield_now().await;
        }

        // Drain the queue so the parked task can finish instead of leaking past the test.
        let _ = outbound_rx.recv().await;
        let _ = outbound_rx.recv().await;
    }
}
