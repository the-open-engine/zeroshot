//! Host retirement joins requests and writers even when the peer stays connected.

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use openengine_cluster_protocol::{GetParams, GetResult, InitializeParams, InitializeResult, RunId};
use crate::watch::fixtures::{fixture_binding, FixtureBackend, FixtureStore};
use crate::{BackendError, ConnectionContext};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use openengine_cluster_testkit::assertions::AssertValue;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio::task::JoinHandle;
use tokio::sync::{Notify, oneshot};

const GET_REQUEST: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"get\",\"params\":{}}\n";

fn backend() -> FixtureBackend {
    FixtureBackend::new(Arc::new(FixtureStore::new(
        RunId::new("shutdown"),
        vec![],
        8,
    )))
}

fn spawn<B: ClusterBackend, W: AsyncWrite + Unpin + Send + 'static>(
    backend: B,
    writer: W,
) -> (
    DuplexStream,
    oneshot::Sender<()>,
    JoinHandle<io::Result<()>>,
) {
    let (client, reader) = tokio::io::duplex(1024);
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(serve_ndjson_until_shutdown(
        fixture_binding(backend),
        NdjsonIo::new(reader, writer, tokio::io::sink()),
        async {
            let _ = stopped.await;
        },
    ));
    (client, stop, task)
}

async fn retire(stop: oneshot::Sender<()>, task: JoinHandle<io::Result<()>>) {
    stop.send(()).assert_value();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .assert_value_with("host shutdown must not wait for an idle peer")
        .assert_value()
        .assert_value();
}

#[tokio::test]
async fn host_shutdown_interrupts_an_idle_reader() {
    let (writer, mut output) = tokio::io::duplex(1024);
    let (mut peer, stop, task) = spawn(backend(), writer);
    peer.write_all(GET_REQUEST).await.assert_value();
    // A response proves binding and dispatch completed while the input remains open.
    tokio::time::timeout(Duration::from_secs(2), output.read_u8())
        .await
        .assert_value()
        .assert_value();
    retire(stop, task).await;
}

#[derive(Default)]
struct Probe {
    entered: Notify,
    dropped: AtomicBool,
}

impl Probe {
    async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.entered.notified())
            .await
            .assert_value();
    }

    fn assert_stopped(&self) {
        assert!(
            self.dropped.load(Ordering::SeqCst),
            "connection work outlived shutdown"
        );
    }
}

struct PendingGet(Arc<Probe>);

struct RequestGuard(Arc<Probe>);

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.0.dropped.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl ClusterBackend for PendingGet {
    async fn initialize(
        &self,
        context: &ConnectionContext,
        params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        backend().initialize(context, params).await
    }

    async fn get(
        &self,
        _context: &ConnectionContext,
        _params: GetParams,
    ) -> Result<GetResult, BackendError> {
        let _owned = RequestGuard(self.0.clone());
        self.0.entered.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn host_shutdown_joins_started_nested_requests_before_returning() {
    let probe = Arc::new(Probe::default());
    let (mut peer, stop, task) = spawn(PendingGet(probe.clone()), tokio::io::sink());
    peer.write_all(GET_REQUEST).await.assert_value();
    probe.wait().await;
    retire(stop, task).await;
    probe.assert_stopped();
}

struct BlockedWriter(Arc<Probe>);

impl AsyncWrite for BlockedWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.0.entered.notify_one();
        Poll::Pending
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Drop for BlockedWriter {
    fn drop(&mut self) {
        self.0.dropped.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn host_shutdown_interrupts_a_blocked_writer_after_input_eof() {
    let probe = Arc::new(Probe::default());
    let (mut peer, stop, task) = spawn(backend(), BlockedWriter(probe.clone()));
    peer.write_all(b"{\n").await.assert_value();
    peer.shutdown().await.assert_value();
    probe.wait().await;
    retire(stop, task).await;
    probe.assert_stopped();
}
